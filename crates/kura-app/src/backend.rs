//! The bridge between the UI and the SDK.
//!
//! The UI sends commands, the worker answers with events. The SDK never runs
//! on the UI thread and the UI never waits for the network.

use std::sync::Arc;

use kura_core::{
    CoreError, LoginOutcome, TwoFactorChallenge, TwoFactorMethod, Vault, VaultSnapshot,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use zeroize::Zeroizing;

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

    while let Some(command) = commands.recv().await {
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
                        continue;
                    }
                };

                match vault.login(&email, &password).await {
                    Ok(LoginOutcome::LoggedIn) => {
                        session = Some(Session {
                            vault,
                            email,
                            password: None,
                        });
                        let _ = events.send(Event::LoggedIn);
                        if let Some(session) = session.as_ref() {
                            refresh(session, false, &events).await;
                        }
                    }
                    Ok(LoginOutcome::TwoFactorRequired(challenge)) => {
                        session = Some(Session {
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
                    continue;
                };
                let Some(password) = current.password.as_ref() else {
                    let _ = events.send(Event::Failed(CoreError::Internal));
                    continue;
                };

                match current
                    .vault
                    .submit_two_factor(&current.email, password, &method, &code)
                    .await
                {
                    Ok(()) => {
                        current.password = None;
                        let _ = events.send(Event::LoggedIn);
                        refresh(current, false, &events).await;
                    }
                    Err(err) => {
                        let _ = events.send(Event::Failed(err));
                    }
                }
            }

            Command::SendEmailCode => {
                let Some(current) = session.as_ref() else {
                    let _ = events.send(Event::Failed(CoreError::Internal));
                    continue;
                };
                let Some(password) = current.password.as_ref() else {
                    let _ = events.send(Event::Failed(CoreError::Internal));
                    continue;
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
                session = None;
            }

            Command::Sync => {
                let Some(current) = session.as_ref() else {
                    let _ = events.send(Event::Failed(CoreError::Internal));
                    continue;
                };
                refresh(current, true, &events).await;
            }
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
