//! The bridge between the UI and the SDK.
//!
//! The UI sends commands, the worker answers with events. The SDK never runs
//! on the UI thread and the UI never waits for the network.

use std::sync::Arc;
use std::time::Duration;

use kura_core::{
    CoreError, LoginOutcome, TotpCode, TwoFactorChallenge, TwoFactorMethod, Vault, VaultItemDetail,
    VaultSnapshot,
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
    pub fn spawn(handle: &tokio::runtime::Handle) -> (Self, UnboundedReceiver<Event>) {
        let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel();
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel = Arc::new(Notify::new());
        handle.spawn(worker(command_rx, event_tx, cancel.clone()));
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
                handle(command, &mut session, &events, &cancel).await;
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
    events: &UnboundedSender<Event>,
    cancel: &Notify,
) {
    match command {
        Command::Login {
            server_url,
            email,
            password,
        } => {
            let vault = match Vault::connect(Some(&server_url)) {
                Ok(vault) => vault,
                Err(err) => {
                    let _ = events.send(Event::Failed(err));
                    return;
                }
            };

            match vault.login(&email, &password).await {
                Ok(LoginOutcome::LoggedIn) => {
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
                    current.password = None;
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
    if let Err(err) = session.vault.sync(force).await {
        let _ = events.send(Event::Failed(err));
        return;
    }
    match session.vault.snapshot().await {
        Ok(snapshot) => {
            let _ = events.send(Event::Snapshot(Arc::new(snapshot)));
        }
        Err(err) => {
            let _ = events.send(Event::Failed(err));
        }
    }
}
