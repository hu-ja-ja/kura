//! The bridge between the UI and the SDK.
//!
//! The UI sends commands, the worker answers with events. The SDK never runs
//! on the UI thread and the UI never waits for the network.

use std::sync::Arc;
use std::time::Duration;

use kura_core::{
    CoreError, LoginOutcome, SavedSession, TotpCode, TwoFactorChallenge, TwoFactorMethod, Vault,
    VaultItemDetail, VaultSnapshot,
};
use tokio::sync::Notify;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::time::{MissedTickBehavior, interval};
use zeroize::Zeroizing;

/// How often an unlocked vault syncs on its own. Override for development.
fn sync_interval() -> Duration {
    let secs = std::env::var("KURA_SYNC_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .unwrap_or(15 * 60);
    Duration::from_secs(secs)
}

pub enum Command {
    Login {
        server_url: String,
        email: String,
        password: String,
    },
    SubmitTwoFactor {
        method: TwoFactorMethod,
        code: String,
    },
    SendEmailCode,
    /// Give up on the login and drop the session.
    Cancel,
    /// Pull the vault again, ignoring the revision date.
    Sync,
    /// Decrypt one item for the detail pane.
    Detail {
        id: String,
    },
    /// Generate the current TOTP for the open item.
    Totp {
        id: String,
    },
    /// Copy a login's password straight from the list.
    CopyPassword {
        id: String,
    },
}

pub enum Event {
    /// The vault is open.
    LoggedIn,
    /// A second factor is needed.
    TwoFactorRequired(TwoFactorChallenge),
    /// A fresh list of items.
    Snapshot(Arc<VaultSnapshot>),
    /// The server sent a code to the address in the challenge.
    EmailCodeSent,
    /// A decrypted item for the detail pane.
    Detail(Box<VaultItemDetail>),
    /// A fresh TOTP for the open item.
    Totp {
        id: String,
        totp: TotpCode,
    },
    /// The detail request failed. The list is unaffected.
    DetailFailed {
        id: String,
        err: CoreError,
    },
    /// The TOTP request failed. The detail pane keeps the old item.
    TotpFailed {
        id: String,
        err: CoreError,
    },
    /// A password for the list's copy button.
    PasswordCopied(Result<String, CoreError>),
    Failed(CoreError),
}

pub struct Backend {
    commands: UnboundedSender<Command>,
    /// Wakes a worker that is waiting on the network so a lock can drop the
    /// session without waiting for the request to finish.
    cancel: Arc<Notify>,
}

impl Backend {
    /// Start the worker and hand back the channel its events arrive on.
    /// `saved` is the login kept from the last sign-in, if any.
    pub fn spawn(
        handle: &tokio::runtime::Handle,
        saved: Option<SavedSession>,
    ) -> (Self, UnboundedReceiver<Event>) {
        let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel();
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel = Arc::new(Notify::new());
        handle.spawn(worker(command_rx, event_tx, cancel.clone(), saved));
        (
            Self {
                commands: command_tx,
                cancel,
            },
            event_rx,
        )
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    /// Drop the session now, even when the worker is mid-request.
    pub fn cancel(&self) {
        self.cancel.notify_waiters();
        self.send(Command::Cancel);
    }
}

/// What the worker keeps between commands.
struct Session {
    vault: Vault,
    email: String,
    /// Only held while a second factor is still expected.
    password: Option<Zeroizing<String>>,
}

async fn worker(
    mut commands: UnboundedReceiver<Command>,
    events: UnboundedSender<Event>,
    cancel: Arc<Notify>,
    mut saved: Option<SavedSession>,
) {
    let mut session: Option<Session> = None;
    let mut ticker = interval(sync_interval());
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // The first tick is immediate; skip it.
    ticker.tick().await;

    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else {
                    break;
                };
                handle(command, &mut session, &mut saved, &events, &cancel).await;
            }
            _ = ticker.tick() => {
                let cancelled = if let Some(current) = session.as_ref()
                    && current.vault.is_unlocked()
                {
                    tokio::select! {
                        _ = cancel.notified() => true,
                        _ = refresh(current, false, &events) => false,
                    }
                } else {
                    false
                };
                if cancelled {
                    session = None;
                }
            }
        }
    }
}

async fn handle(
    command: Command,
    session: &mut Option<Session>,
    saved: &mut Option<SavedSession>,
    events: &UnboundedSender<Event>,
    cancel: &Notify,
) {
    match command {
        Command::Login {
            server_url,
            email,
            password,
        } => {
            // A saved login answers the password locally: no sign-in round trip
            // and no second factor, since the server already accepted both.
            if let Some(data) = saved
                .as_ref()
                .filter(|data| data.matches(&server_url, &email))
            {
                match Vault::restore(data, &password).await {
                    Ok(vault) => {
                        match vault.sync(false).await {
                            Ok(()) => {
                                *session = Some(Session {
                                    vault,
                                    email,
                                    password: None,
                                });
                                let _ = events.send(Event::LoggedIn);
                                if let Some(current) = session.as_ref()
                                    && let Err(err) = publish(current, events).await
                                {
                                    let _ = events.send(Event::Failed(err));
                                }
                                return;
                            }
                            // The token was revoked or expired: sign in again.
                            Err(CoreError::Rejected(_)) => {
                                *saved = None;
                                crate::session::clear();
                            }
                            Err(err) => {
                                let _ = events.send(Event::Failed(err));
                                return;
                            }
                        }
                    }
                    // A typo, or a master password changed on another device.
                    // Only the server can tell them apart, so sign in and let it
                    // answer; a changed password replaces the saved login.
                    Err(CoreError::InvalidCredentials) => {}
                    Err(err) => {
                        let _ = events.send(Event::Failed(err));
                        return;
                    }
                }
            }

            let vault = match Vault::connect(Some(&server_url)) {
                Ok(vault) => vault,
                Err(err) => {
                    let _ = events.send(Event::Failed(err));
                    return;
                }
            };

            match vault.login(&email, &password).await {
                Ok(LoginOutcome::LoggedIn) => {
                    remember(&vault, saved, &email, &password).await;
                    *session = Some(Session {
                        vault,
                        email,
                        password: None,
                    });
                    let _ = events.send(Event::LoggedIn);
                    if let Some(current) = session.as_ref() {
                        refresh(current, false, events).await;
                    }
                }
                Ok(LoginOutcome::TwoFactorRequired(challenge)) => {
                    *session = Some(Session {
                        vault,
                        email,
                        password: Some(Zeroizing::new(password)),
                    });
                    let _ = events.send(Event::TwoFactorRequired(challenge));
                }
                Err(err) => {
                    let _ = events.send(Event::Failed(err));
                }
            }
        }

        Command::SubmitTwoFactor { method, code } => {
            let Some(current) = session.as_mut() else {
                let _ = events.send(Event::Failed(CoreError::Internal));
                return;
            };
            let Some(password) = current.password.as_ref() else {
                let _ = events.send(Event::Failed(CoreError::Internal));
                return;
            };

            match current
                .vault
                .submit_two_factor(&current.email, password, &method, &code)
                .await
            {
                Ok(()) => {
                    if let Some(password) = current.password.take() {
                        remember(&current.vault, saved, &current.email, &password).await;
                    }
                    let _ = events.send(Event::LoggedIn);
                    refresh(current, false, events).await;
                }
                Err(err) => {
                    let _ = events.send(Event::Failed(err));
                }
            }
        }

        Command::SendEmailCode => {
            let Some(current) = session.as_ref() else {
                let _ = events.send(Event::Failed(CoreError::Internal));
                return;
            };
            let Some(password) = current.password.as_ref() else {
                let _ = events.send(Event::Failed(CoreError::Internal));
                return;
            };

            match current
                .vault
                .send_email_code(&current.email, password)
                .await
            {
                Ok(()) => {
                    let _ = events.send(Event::EmailCodeSent);
                }
                Err(err) => {
                    let _ = events.send(Event::Failed(err));
                }
            }
        }

        Command::Cancel => {
            *session = None;
        }

        Command::Sync => {
            let cancelled = {
                let Some(current) = session.as_ref() else {
                    let _ = events.send(Event::Failed(CoreError::Internal));
                    return;
                };
                tokio::select! {
                    _ = cancel.notified() => true,
                    _ = refresh(current, true, events) => false,
                }
            };
            if cancelled {
                *session = None;
            }
        }

        Command::Detail { id } => {
            let result = {
                let Some(current) = session.as_ref() else {
                    let _ = events.send(Event::DetailFailed {
                        id,
                        err: CoreError::Internal,
                    });
                    return;
                };
                tokio::select! {
                    _ = cancel.notified() => None,
                    result = current.vault.detail(&id) => Some(result),
                }
            };
            let Some(result) = result else {
                *session = None;
                return;
            };
            match result {
                Ok(detail) => {
                    let _ = events.send(Event::Detail(Box::new(detail)));
                }
                Err(err) => {
                    let _ = events.send(Event::DetailFailed { id, err });
                }
            }
        }

        Command::Totp { id } => {
            let result = {
                let Some(current) = session.as_ref() else {
                    let _ = events.send(Event::TotpFailed {
                        id,
                        err: CoreError::Internal,
                    });
                    return;
                };
                tokio::select! {
                    _ = cancel.notified() => None,
                    result = current.vault.totp(&id) => Some(result),
                }
            };
            let Some(result) = result else {
                *session = None;
                return;
            };
            match result {
                Ok(totp) => {
                    let _ = events.send(Event::Totp { id, totp });
                }
                Err(err) => {
                    let _ = events.send(Event::TotpFailed { id, err });
                }
            }
        }

        Command::CopyPassword { id } => {
            let result = {
                let Some(current) = session.as_ref() else {
                    let _ = events.send(Event::PasswordCopied(Err(CoreError::Internal)));
                    return;
                };
                tokio::select! {
                    _ = cancel.notified() => None,
                    result = current.vault.password(&id) => Some(result),
                }
            };
            let Some(result) = result else {
                *session = None;
                return;
            };
            let _ = events.send(Event::PasswordCopied(result));
        }
    }
}

/// Sync, then hand over the new list.
async fn refresh(session: &Session, force: bool, events: &UnboundedSender<Event>) {
    let result = match session.vault.sync(force).await {
        Ok(()) => publish(session, events).await,
        Err(err) => Err(err),
    };
    if let Err(err) = result {
        let _ = events.send(Event::Failed(err));
    }
}

/// Hand the current list to the UI. The caller syncs first.
async fn publish(session: &Session, events: &UnboundedSender<Event>) -> Result<(), CoreError> {
    let snapshot = session.vault.snapshot().await?;
    let _ = events.send(Event::Snapshot(Arc::new(snapshot)));
    Ok(())
}

/// Keep the login so the next start needs only the master password.
async fn remember(vault: &Vault, saved: &mut Option<SavedSession>, email: &str, password: &str) {
    let Ok(data) = vault.saved_session(email, password).await else {
        return;
    };
    let _ = crate::session::save(&data);
    *saved = Some(data);
}
