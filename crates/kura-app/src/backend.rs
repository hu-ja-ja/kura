//! The bridge between the UI and the SDK.
//!
//! The UI sends commands, the worker answers with events. The SDK never runs
//! on the UI thread and the UI never waits for the network.

use std::sync::Arc;
use std::time::Duration;

use kura_core::{
    CoreError, LoginOutcome, TwoFactorChallenge, TwoFactorMethod, Vault, VaultSnapshot,
};
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
    Failed(CoreError),
}

pub struct Backend {
    commands: UnboundedSender<Command>,
}

impl Backend {
    /// Start the worker and hand back the channel its events arrive on.
    pub fn spawn(handle: &tokio::runtime::Handle) -> (Self, UnboundedReceiver<Event>) {
        let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel();
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        handle.spawn(worker(command_rx, event_tx));
        (
            Self {
                commands: command_tx,
            },
            event_rx,
        )
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

/// What the worker keeps between commands.
struct Session {
    vault: Vault,
    email: String,
    /// Only held while a second factor is still expected.
    password: Option<Zeroizing<String>>,
}

async fn worker(mut commands: UnboundedReceiver<Command>, events: UnboundedSender<Event>) {
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
                handle(command, &mut session, &events).await;
            }
            _ = ticker.tick() => {
                if let Some(current) = session.as_ref()
                    && current.vault.is_unlocked()
                {
                    refresh(current, false, &events).await;
                }
            }
        }
    }
}

async fn handle(command: Command, session: &mut Option<Session>, events: &UnboundedSender<Event>) {
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
            let Some(current) = session.as_ref() else {
                let _ = events.send(Event::Failed(CoreError::Internal));
                return;
            };
            refresh(current, true, events).await;
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
