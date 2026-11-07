//! The bridge between the UI and the SDK.
//!
//! The UI sends commands, the worker answers with events. The SDK never runs
//! on the UI thread and the UI never waits for the network.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

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
        password: Zeroizing<String>,
        /// The base URL the user approved for this sign-in. The password only
        /// leaves the machine when it matches where the sign-in would go.
        confirmed_server: Option<String>,
    },
    SubmitTwoFactor {
        method: TwoFactorMethod,
        code: String,
    },
    SendEmailCode,
    /// Give up on the login. A vault that is only retained for the next
    /// unlock stays in memory.
    Cancel,
    /// Forget that the user approved a server, so the next sign-in asks again.
    ForgetServer {
        server: String,
    },
    /// Remove the saved login from disk and from the worker's memory. A later
    /// sync or lock must not write it back.
    ForgetSavedLogin,
    /// Drop the session because the vault was locked.
    Lock,
    /// Pull the vault again, ignoring the revision date.
    Sync,
    /// Show the list that is already in memory, then refresh it in the
    /// background. Used when a window is created for an open vault.
    Snapshot,
    /// Refresh an open vault in the background without rebuilding the list.
    Refresh,
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
    /// The session was dropped, whether by the user or by an automatic lock.
    Locked,
    /// The master password would go to a server the user has not approved yet.
    /// Nothing has been sent; the UI names the base URL and asks.
    ConfirmServer {
        server: String,
    },
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
    PasswordCopied(Result<Zeroizing<String>, CoreError>),
    /// The login worked but the saved login could not be written. The next
    /// start needs the full sign-in again.
    SessionNotSaved,
    /// The worker removed the saved login from memory. False means the file is
    /// still on disk; the detail is in the log.
    SavedLoginForgotten(bool),
    Failed(CoreError),
}

#[derive(Clone)]
pub struct Backend {
    commands: UnboundedSender<Command>,
    /// Wakes a worker that is waiting on the network so a lock can drop the
    /// session without waiting for the request to finish.
    cancel: Arc<Notify>,
    /// Whether the worker currently holds an unlocked vault. Read when the
    /// window is recreated after sitting in the tray.
    unlocked: Arc<AtomicBool>,
}

impl Backend {
    /// Start the worker and hand back the channel its events arrive on.
    /// `saved` is the login kept from the last sign-in, if any, and
    /// `confirmed_servers` are the base URLs the user has already approved for
    /// the master password.
    pub fn spawn(
        handle: &tokio::runtime::Handle,
        saved: Option<SavedSession>,
        confirmed_servers: Vec<String>,
    ) -> (Self, UnboundedReceiver<Event>) {
        let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel();
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel = Arc::new(Notify::new());
        let unlocked = Arc::new(AtomicBool::new(false));
        handle.spawn(worker(
            command_rx,
            event_tx,
            cancel.clone(),
            unlocked.clone(),
            saved,
            confirmed_servers,
        ));
        (
            Self {
                commands: command_tx,
                cancel,
                unlocked,
            },
            event_rx,
        )
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    /// Drop the sign-in session now, even when the worker is mid-request. A
    /// vault retained for the next unlock stays in memory.
    pub fn cancel(&self) {
        self.cancel.notify_waiters();
        self.send(Command::Cancel);
    }

    /// Lock the vault and tell the UI, whether a window exists or not.
    pub fn lock(&self) {
        self.cancel.notify_waiters();
        self.send(Command::Lock);
    }

    /// Whether a vault is currently unlocked in the worker.
    pub fn is_unlocked(&self) -> bool {
        self.unlocked.load(Ordering::SeqCst)
    }

    /// A backend whose commands go nowhere, with its event stream kept open so
    /// the controller does not mistake it for a stopped worker.
    pub fn detached(unlocked: bool) -> (Self, tokio::sync::mpsc::UnboundedReceiver<Event>) {
        let (commands, _dropped) = tokio::sync::mpsc::unbounded_channel();
        let (events, events_rx) = tokio::sync::mpsc::unbounded_channel();
        // Nothing sends events here, but the receiver must not close.
        std::mem::forget(events);
        (
            Self {
                commands,
                cancel: Arc::new(Notify::new()),
                unlocked: Arc::new(AtomicBool::new(unlocked)),
            },
            events_rx,
        )
    }
}

/// What the worker keeps between commands.
struct Session {
    /// Shared with a running sync so the task outlives a command, not the vault.
    vault: Arc<Vault>,
    email: String,
    /// Only held while a second factor is still expected.
    password: Option<Zeroizing<String>>,
}

/// Whether Cancel should drop this session. A vault that was only retained for
/// the next unlock is kept; a pending second factor or an open vault belongs to
/// the sign-in flow.
fn cancel_drops(session: &Session) -> bool {
    session.password.is_some() || session.vault.is_unlocked()
}

/// Which kind of background sync finished.
#[derive(Clone, Copy)]
enum SyncKind {
    /// The sync that follows an unlock. The master password is gone, so a
    /// rejected token has to send the user back to the sign-in form.
    AfterUnlock,
    /// A refresh of an already open vault. The local list stays usable.
    Refresh,
}

/// A background sync the worker has to finish: renew the saved tokens and,
/// when something changed, hand the new list to the UI.
struct Synced {
    vault: Arc<Vault>,
    kind: SyncKind,
    result: Result<bool, CoreError>,
    elapsed: Duration,
}

/// Run a sync off the worker so a detail click does not wait for the network.
/// One at a time is enough; the running request reads the same server state.
fn spawn_sync(
    vault: Arc<Vault>,
    kind: SyncKind,
    syncing: &Arc<AtomicBool>,
    done: &UnboundedSender<Synced>,
) {
    if syncing.swap(true, Ordering::SeqCst) {
        return;
    }
    let syncing = syncing.clone();
    let done = done.clone();
    tokio::spawn(async move {
        let started = Instant::now();
        let result = vault.sync(false).await;
        syncing.store(false, Ordering::SeqCst);
        let _ = done.send(Synced {
            vault,
            kind,
            result,
            elapsed: started.elapsed(),
        });
    });
}

/// Whether the master password may go to `server`.
///
/// A custom server (its full base URL, including any path) is allowed once the
/// user has approved it, either with this sign-in or with an earlier one in
/// the same run. The check sits at the send site so a new caller cannot
/// bypass it.
fn server_approved(server: &str, confirmed: Option<&str>, known: &[String]) -> bool {
    confirmed == Some(server) || known.iter().any(|known| known == server)
}

/// A log line for a failed command. The code is stable and the step label is
/// static, so neither carries server text or a vault value.
fn failure_summary(context: &str, err: &CoreError) -> String {
    match err.context() {
        Some(step) => format!("{context} failed: {} ({step})", err.code()),
        None => format!("{context} failed: {}", err.code()),
    }
}

fn log_failure(context: &str, err: &CoreError) {
    log::warn!("{}", failure_summary(context, err));
}

async fn worker(
    mut commands: UnboundedReceiver<Command>,
    events: UnboundedSender<Event>,
    cancel: Arc<Notify>,
    unlocked: Arc<AtomicBool>,
    mut saved: Option<SavedSession>,
    mut confirmed_servers: Vec<String>,
) {
    let mut session: Option<Session> = None;
    let (synced_tx, mut synced_rx) = tokio::sync::mpsc::unbounded_channel();
    let syncing = Arc::new(AtomicBool::new(false));
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
                handle(
                    command,
                    &mut session,
                    &mut saved,
                    &mut confirmed_servers,
                    &events,
                    &cancel,
                    &unlocked,
                    &syncing,
                    &synced_tx,
                )
                .await;
            }
            Some(synced) = synced_rx.recv() => {
                finish_sync(&mut session, &mut saved, synced, &events, &unlocked).await;
            }
            _ = ticker.tick() => {
                if let Some(current) = session.as_ref()
                    && current.vault.is_unlocked()
                {
                    spawn_sync(
                        Arc::clone(&current.vault),
                        SyncKind::Refresh,
                        &syncing,
                        &synced_tx,
                    );
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle(
    command: Command,
    session: &mut Option<Session>,
    saved: &mut Option<SavedSession>,
    confirmed_servers: &mut Vec<String>,
    events: &UnboundedSender<Event>,
    cancel: &Notify,
    unlocked: &AtomicBool,
    syncing: &Arc<AtomicBool>,
    synced: &UnboundedSender<Synced>,
) {
    match command {
        Command::Login {
            server_url,
            email,
            password,
            confirmed_server,
        } => {
            // A saved login answers the password locally: no sign-in round trip
            // and no second factor, since the server already accepted both.
            let mut drop_saved = false;
            if let Some(data) = saved
                .as_ref()
                .filter(|data| data.matches(&server_url, &email))
            {
                // A locked vault still holds its encrypted items. Reuse it when
                // it belongs to this account so the master password only opens
                // the keys again and the list comes from memory.
                let reuse = session.as_ref().is_some_and(|current| {
                    current.password.is_none()
                        && !current.vault.is_unlocked()
                        && current.email.eq_ignore_ascii_case(data.email.trim())
                        && current.vault.owns(data, &email)
                });
                let opened_started = Instant::now();
                let opened = if reuse {
                    let Some(current) = session.as_ref() else {
                        return;
                    };
                    let result = tokio::select! {
                        _ = cancel.notified() => None,
                        result = current.vault.unlock(data, &password) => Some(result),
                    };
                    let Some(result) = result else {
                        return;
                    };
                    result.map(|()| None)
                } else {
                    // A vault for another account cannot answer this login.
                    *session = None;
                    let result = tokio::select! {
                        _ = cancel.notified() => None,
                        result = Vault::restore(data, &password) => Some(result),
                    };
                    let Some(result) = result else {
                        return;
                    };
                    result.map(Some)
                };

                match opened {
                    Ok(vault) => {
                        log::info!(
                            "{} the saved login in {} ms",
                            if reuse { "unlocked" } else { "restored" },
                            opened_started.elapsed().as_millis()
                        );
                        if let Some(vault) = vault {
                            *session = Some(Session {
                                vault: Arc::new(vault),
                                email: email.clone(),
                                password: None,
                            });
                        }
                        unlocked.store(true, Ordering::SeqCst);
                        let _ = events.send(Event::LoggedIn);
                        log::info!("opened the vault from the saved login");

                        if reuse {
                            let current = session
                                .as_ref()
                                .expect("the retained vault answered the login");
                            // The retained list is already here; show it
                            // before the network round trip.
                            if let Err(err) = publish(current, events).await {
                                log_failure("publish the retained vault", &err);
                                let _ = events.send(Event::Failed(err));
                            }
                            let vault = Arc::clone(&current.vault);
                            spawn_sync(vault, SyncKind::AfterUnlock, syncing, synced);
                            return;
                        }

                        let result = {
                            let current = session.as_ref().expect("the login installed the vault");
                            tokio::select! {
                                _ = cancel.notified() => return,
                                result = sync_and_publish(current, saved, false, events) => result,
                            }
                        };

                        match result {
                            Ok(()) => return,
                            // The token is no longer accepted. Drop the local
                            // copy and let the sign-in ask for what the server
                            // wants now; the password is still in hand.
                            Err(err @ (CoreError::Rejected(_) | CoreError::AccountDeleted)) => {
                                log_failure("sync the opened vault", &err);
                                *session = None;
                                unlocked.store(false, Ordering::SeqCst);
                                let _ = events.send(Event::Locked);
                                drop_saved = true;
                            }
                            Err(err) => {
                                log_failure("sync the opened vault", &err);
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
                        log_failure("open the saved login", &err);
                        let _ = events.send(Event::Failed(err));
                        return;
                    }
                }
            }

            if drop_saved {
                *saved = None;
                if let Err(err) = crate::session::clear() {
                    log::warn!("could not remove the saved login: {err}");
                }
            }

            // Everything above stays on this machine. From here the master
            // password goes over the network, so the target has to be one the
            // user has seen and approved.
            let target = match kura_core::sign_in_base(Some(&server_url)) {
                Ok(target) => target,
                Err(err) => {
                    log_failure("resolve the server", &err);
                    let _ = events.send(Event::Failed(err));
                    return;
                }
            };
            match &target {
                None => {}
                Some(server) => {
                    if !server_approved(server, confirmed_server.as_deref(), confirmed_servers) {
                        let _ = events.send(Event::ConfirmServer {
                            server: server.clone(),
                        });
                        return;
                    }
                    if !confirmed_servers.iter().any(|known| known == server) {
                        confirmed_servers.push(server.clone());
                    }
                }
            }

            let vault = match Vault::connect(Some(&server_url)) {
                Ok(vault) => vault,
                Err(err) => {
                    log_failure("connect to the server", &err);
                    let _ = events.send(Event::Failed(err));
                    return;
                }
            };

            let outcome = tokio::select! {
                _ = cancel.notified() => return,
                result = vault.login(&email, &password) => result,
            };
            match outcome {
                Ok(LoginOutcome::LoggedIn) => {
                    let remembered = remember(&vault, saved, &email, &password).await;
                    *session = Some(Session {
                        vault: Arc::new(vault),
                        email,
                        password: None,
                    });
                    unlocked.store(true, Ordering::SeqCst);
                    let _ = events.send(Event::LoggedIn);
                    log::info!("signed in");
                    if !remembered {
                        let _ = events.send(Event::SessionNotSaved);
                    }
                    if let Some(current) = session.as_ref() {
                        // A cancel here is a lock or a trip to the tray; the
                        // command that follows decides the vault's fate.
                        tokio::select! {
                            _ = cancel.notified() => {}
                            _ = refresh(current, saved, false, events) => {}
                        }
                    }
                }
                Ok(LoginOutcome::TwoFactorRequired(challenge)) => {
                    *session = Some(Session {
                        vault: Arc::new(vault),
                        email,
                        password: Some(password),
                    });
                    unlocked.store(false, Ordering::SeqCst);
                    let _ = events.send(Event::TwoFactorRequired(challenge));
                }
                Err(err) => {
                    log_failure("sign in", &err);
                    let _ = events.send(Event::Failed(err));
                }
            }
        }

        Command::SubmitTwoFactor { method, code } => {
            let Some(current) = session.as_mut() else {
                let err = CoreError::Internal("the two-factor challenge is no longer open");
                log_failure("submit the two-factor code", &err);
                let _ = events.send(Event::Failed(err));
                return;
            };
            let Some(password) = current.password.as_ref() else {
                let err = CoreError::Internal("the two-factor challenge has no password");
                log_failure("submit the two-factor code", &err);
                let _ = events.send(Event::Failed(err));
                return;
            };

            let result = tokio::select! {
                _ = cancel.notified() => return,
                result = current
                    .vault
                    .submit_two_factor(&current.email, password, &method, &code) => result,
            };
            match result {
                Ok(()) => {
                    let mut remembered = true;
                    if let Some(password) = current.password.take() {
                        remembered =
                            remember(&current.vault, saved, &current.email, &password).await;
                    }
                    let _ = events.send(Event::LoggedIn);
                    log::info!("signed in");
                    if !remembered {
                        let _ = events.send(Event::SessionNotSaved);
                    }
                    unlocked.store(true, Ordering::SeqCst);
                    // A cancel here is a lock or a trip to the tray; the command
                    // that follows decides the vault's fate.
                    tokio::select! {
                        _ = cancel.notified() => {}
                        _ = refresh(current, saved, false, events) => {}
                    }
                }
                Err(err) => {
                    log_failure("submit the two-factor code", &err);
                    let _ = events.send(Event::Failed(err));
                }
            }
        }

        Command::SendEmailCode => {
            let Some(current) = session.as_ref() else {
                let err = CoreError::Internal("the two-factor challenge is no longer open");
                log_failure("send the email code", &err);
                let _ = events.send(Event::Failed(err));
                return;
            };
            let Some(password) = current.password.as_ref() else {
                let err = CoreError::Internal("the two-factor challenge has no password");
                log_failure("send the email code", &err);
                let _ = events.send(Event::Failed(err));
                return;
            };

            let result = tokio::select! {
                _ = cancel.notified() => return,
                result = current
                    .vault
                    .send_email_code(&current.email, password) => result,
            };
            match result {
                Ok(()) => {
                    let _ = events.send(Event::EmailCodeSent);
                }
                Err(err) => {
                    log_failure("send the email code", &err);
                    let _ = events.send(Event::Failed(err));
                }
            }
        }

        Command::Cancel => {
            // Keep a vault that is only retained for the next unlock. A pending
            // second factor or an open vault belongs to the sign-in flow.
            if session.as_ref().is_some_and(cancel_drops) {
                *session = None;
            }
            unlocked.store(false, Ordering::SeqCst);
            // The UI may already have reset itself, but a LoggedIn event that
            // was queued before this command must not reopen the vault.
            let _ = events.send(Event::Locked);
        }

        Command::ForgetServer { server } => {
            confirmed_servers.retain(|known| known != &server);
        }

        Command::ForgetSavedLogin => {
            // Drop the in-memory copy first: a sync or lock that runs later
            // would otherwise write the file back.
            *saved = None;
            let removed = match crate::session::clear() {
                Ok(()) => true,
                Err(err) => {
                    log::warn!("could not remove the saved login: {err}");
                    false
                }
            };
            // A locked copy cannot be opened without the login that was just
            // removed. An open vault is still in use.
            if session
                .as_ref()
                .is_some_and(|current| !current.vault.is_unlocked())
            {
                *session = None;
            }
            let _ = events.send(Event::SavedLoginForgotten(removed));
        }

        Command::Lock => {
            // A half-finished sign-in still holds the master password.
            let pending_second_factor = session
                .as_ref()
                .is_some_and(|current| current.password.is_some() && !current.vault.is_unlocked());
            if pending_second_factor {
                *session = None;
            } else if let Some(current) = session.as_ref()
                && current.vault.is_unlocked()
            {
                // A request may have renewed the tokens since the last sync.
                // Keep the freshest pair before the keys are dropped.
                keep_saved_tokens(&current.vault, &current.email, saved).await;
                current.vault.lock().await;
                // The client and its encrypted items stay for the next unlock.
            }
            // Without a saved login that matches this vault, nothing can open
            // the retained client again, so do not keep it.
            if session.as_ref().is_some_and(|current| {
                !current.vault.is_unlocked()
                    && !saved
                        .as_ref()
                        .is_some_and(|data| current.vault.owns(data, &current.email))
            }) {
                *session = None;
            }
            unlocked.store(false, Ordering::SeqCst);
            log::info!("the vault is locked");
            let _ = events.send(Event::Locked);
        }

        Command::Sync => {
            let Some(current) = session.as_ref() else {
                let err = CoreError::Internal("sync without an open vault");
                log_failure("sync", &err);
                let _ = events.send(Event::Failed(err));
                return;
            };
            if !current.vault.is_unlocked() {
                // A locked vault is refreshed by the unlock, not by this command.
                return;
            }
            tokio::select! {
                _ = cancel.notified() => {}
                _ = refresh(current, saved, true, events) => {}
            }
        }

        Command::Snapshot => {
            let Some(current) = session
                .as_ref()
                .filter(|current| current.vault.is_unlocked())
            else {
                return;
            };
            if let Err(err) = publish(current, events).await {
                log_failure("publish the vault", &err);
                let _ = events.send(Event::Failed(err));
                return;
            }
            spawn_sync(
                Arc::clone(&current.vault),
                SyncKind::Refresh,
                syncing,
                synced,
            );
        }

        Command::Refresh => {
            let Some(current) = session
                .as_ref()
                .filter(|current| current.vault.is_unlocked())
            else {
                return;
            };
            spawn_sync(
                Arc::clone(&current.vault),
                SyncKind::Refresh,
                syncing,
                synced,
            );
        }

        Command::Detail { id } => {
            let Some(current) = session
                .as_ref()
                .filter(|current| current.vault.is_unlocked())
            else {
                let err = CoreError::Internal("open a detail without an open vault");
                log_failure("open the detail", &err);
                let _ = events.send(Event::DetailFailed { id, err });
                return;
            };
            let result = tokio::select! {
                _ = cancel.notified() => return,
                result = current.vault.detail(&id) => result,
            };
            match result {
                Ok(detail) => {
                    let _ = events.send(Event::Detail(Box::new(detail)));
                }
                Err(err) => {
                    log_failure("open the detail", &err);
                    let _ = events.send(Event::DetailFailed { id, err });
                }
            }
        }

        Command::Totp { id } => {
            let Some(current) = session
                .as_ref()
                .filter(|current| current.vault.is_unlocked())
            else {
                let err = CoreError::Internal("generate a TOTP without an open vault");
                log_failure("generate the TOTP", &err);
                let _ = events.send(Event::TotpFailed { id, err });
                return;
            };
            let result = tokio::select! {
                _ = cancel.notified() => return,
                result = current.vault.totp(&id) => result,
            };
            match result {
                Ok(totp) => {
                    let _ = events.send(Event::Totp { id, totp });
                }
                // The pane retries every few seconds; logging each failure
                // would flood the file.
                Err(err) => {
                    let _ = events.send(Event::TotpFailed { id, err });
                }
            }
        }

        Command::CopyPassword { id } => {
            let Some(current) = session
                .as_ref()
                .filter(|current| current.vault.is_unlocked())
            else {
                let err = CoreError::Internal("copy a password without an open vault");
                log_failure("copy the password", &err);
                let _ = events.send(Event::PasswordCopied(Err(err)));
                return;
            };
            let result = tokio::select! {
                _ = cancel.notified() => return,
                result = current.vault.password(&id) => result,
            };
            if let Err(err) = &result
                && !matches!(err, CoreError::NoPassword)
            {
                log_failure("copy the password", err);
            }
            let _ = events.send(Event::PasswordCopied(result));
        }
    }
}

/// Apply a finished background sync. A result for a vault that was replaced
/// while the request ran is ignored.
async fn finish_sync(
    session: &mut Option<Session>,
    saved: &mut Option<SavedSession>,
    synced: Synced,
    events: &UnboundedSender<Event>,
    unlocked: &AtomicBool,
) {
    let Some(current) = session.as_ref() else {
        return;
    };
    if !Arc::ptr_eq(&current.vault, &synced.vault) {
        return;
    }
    let email = current.email.clone();
    // Tokens can be renewed without the keys, so keep the file current even
    // when the vault was locked while the request ran.
    keep_saved_tokens(&synced.vault, &email, saved).await;

    match synced.result {
        Ok(true) => {
            log::info!("synced the vault in {} ms", synced.elapsed.as_millis());
            if let Some(current) = session
                .as_ref()
                .filter(|current| current.vault.is_unlocked())
                && let Err(err) = publish(current, events).await
            {
                log_failure("publish the vault", &err);
                let _ = events.send(Event::Failed(err));
            }
        }
        Ok(false) => log::info!("the vault is up to date"),
        Err(err) => {
            log_failure("sync", &err);
            let rejected = matches!(&err, CoreError::Rejected(_) | CoreError::AccountDeleted);
            if rejected {
                // The saved tokens are dead. The next unlock has to sign in.
                if saved.take().is_some()
                    && let Err(err) = crate::session::clear()
                {
                    log::warn!("could not remove the saved login: {err}");
                }
            }
            match synced.kind {
                // The password is gone after an unlock, so the only way back
                // is the sign-in form.
                SyncKind::AfterUnlock if rejected => {
                    *session = None;
                    unlocked.store(false, Ordering::SeqCst);
                    let _ = events.send(Event::Locked);
                }
                _ => {
                    let _ = events.send(Event::Failed(err));
                }
            }
        }
    }
}

/// Sync, then hand over the new list.
///
/// Returns the error so the login path can tell a rejected token from a
/// temporary failure.
async fn sync_and_publish(
    session: &Session,
    saved: &mut Option<SavedSession>,
    force: bool,
    events: &UnboundedSender<Event>,
) -> Result<(), CoreError> {
    let started = Instant::now();
    let result = match session.vault.sync(force).await {
        Ok(true) => {
            log::info!("synced the vault in {} ms", started.elapsed().as_millis());
            publish(session, events).await
        }
        Ok(false) => Ok(()),
        Err(err) => Err(err),
    };
    keep_saved_tokens(&session.vault, &session.email, saved).await;
    result
}

/// Sync, then hand over the new list.
async fn refresh(
    session: &Session,
    saved: &mut Option<SavedSession>,
    force: bool,
    events: &UnboundedSender<Event>,
) {
    if let Err(err) = sync_and_publish(session, saved, force, events).await {
        log_failure("sync", &err);
        let _ = events.send(Event::Failed(err));
    }
}

/// The SDK renews and rotates the tokens in memory during a request. Without
/// this the next start presents a refresh token the server has replaced.
async fn keep_saved_tokens(vault: &Vault, email: &str, saved: &mut Option<SavedSession>) {
    let Some(data) = saved.as_mut() else {
        return;
    };
    // A failed sign-in to another account leaves the old file in place.
    if !vault.owns(data, email) {
        return;
    }
    let previous = data.tokens.clone();
    match vault.update_saved_tokens(data).await {
        Ok(false) => {}
        Ok(true) => {
            log::info!("saved login tokens were renewed");
            if let Err(err) = crate::session::save(data) {
                log::warn!("could not write the saved login: {err}");
                // Keep the old tokens so the next sync sees a difference and
                // tries the write again.
                data.tokens = previous;
            }
        }
        Err(err) => log_failure("update the saved tokens", &err),
    }
}

/// Hand the current list to the UI.
async fn publish(session: &Session, events: &UnboundedSender<Event>) -> Result<(), CoreError> {
    let snapshot = session.vault.snapshot().await?;
    if snapshot.decrypt_failures > 0 {
        log::warn!(
            "{} vault items could not be decrypted",
            snapshot.decrypt_failures
        );
    }
    let _ = events.send(Event::Snapshot(Arc::new(snapshot)));
    Ok(())
}

/// Keep the login so the next start needs only the master password.
async fn remember(
    vault: &Vault,
    saved: &mut Option<SavedSession>,
    email: &str,
    password: &str,
) -> bool {
    let data = match vault.saved_session(email, password).await {
        Ok(data) => data,
        Err(err) => {
            log_failure("prepare the saved login", &err);
            return false;
        }
    };
    if let Err(err) = crate::session::save(&data) {
        log::warn!("could not write the saved login: {err}");
        return false;
    }
    *saved = Some(data);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_custom_server_is_only_used_after_the_user_approved_it() {
        let known = Vec::new();

        assert!(!server_approved("https://vault.example.com", None, &known));
        // Approving one server says nothing about another.
        assert!(!server_approved(
            "https://evil.example",
            Some("https://vault.example.com"),
            &known
        ));
        assert!(server_approved(
            "https://vault.example.com",
            Some("https://vault.example.com"),
            &known
        ));
    }

    #[test]
    fn an_approved_server_stays_approved_for_the_run() {
        let known = vec!["https://vault.example.com".to_string()];

        assert!(server_approved("https://vault.example.com", None, &known));
        assert!(!server_approved("https://evil.example", None, &known));
        assert!(!server_approved(
            "https://vault.example.com:8443",
            None,
            &known
        ));
    }

    #[test]
    fn a_path_change_on_an_approved_server_asks_again() {
        let known = vec!["https://vault.example.com/bitwarden".to_string()];

        assert!(server_approved(
            "https://vault.example.com/bitwarden",
            None,
            &known
        ));
        assert!(!server_approved(
            "https://vault.example.com/other",
            None,
            &known
        ));
        assert!(!server_approved("https://vault.example.com", None, &known));
    }

    #[test]
    fn a_failure_line_keeps_server_text_out_of_the_log() {
        let line = failure_summary(
            "sign in",
            &CoreError::Rejected("the password for user@example.com is wrong".to_string()),
        );

        assert_eq!(line, "sign in failed: rejected");
        assert!(!line.contains("user@example.com"));
        assert!(!line.contains("password"));

        let line = failure_summary(
            "sync",
            &CoreError::Server("a secret-looking body".to_string()),
        );
        assert_eq!(line, "sync failed: server");
        assert!(!line.contains("secret-looking"));
    }

    #[test]
    fn a_failure_line_names_the_internal_step() {
        let line = failure_summary("sync", &CoreError::Internal("read the decrypted items"));

        assert_eq!(line, "sync failed: internal (read the decrypted items)");
    }

    #[test]
    fn cancel_keeps_a_vault_that_is_only_retained_for_the_next_unlock() {
        let retained = Session {
            vault: Arc::new(Vault::connect(None).unwrap()),
            email: "test@example.com".to_string(),
            password: None,
        };
        assert!(!cancel_drops(&retained));

        let second_factor = Session {
            vault: Arc::new(Vault::connect(None).unwrap()),
            email: "test@example.com".to_string(),
            password: Some(Zeroizing::new("a master password".to_string())),
        };
        assert!(cancel_drops(&second_factor));
    }

    #[tokio::test]
    async fn a_sync_for_a_locked_vault_does_not_publish() {
        let vault = Arc::new(Vault::connect(None).unwrap());
        let mut session = Some(Session {
            vault: Arc::clone(&vault),
            email: "test@example.com".to_string(),
            password: None,
        });
        let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
        let mut saved = None;

        finish_sync(
            &mut session,
            &mut saved,
            Synced {
                vault,
                kind: SyncKind::Refresh,
                result: Ok(true),
                elapsed: Duration::ZERO,
            },
            &events,
            &AtomicBool::new(false),
        )
        .await;

        assert!(received.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_rejected_sync_after_unlock_sends_the_user_back_to_the_login() {
        let vault = Arc::new(Vault::connect(None).unwrap());
        let mut session = Some(Session {
            vault: Arc::clone(&vault),
            email: "test@example.com".to_string(),
            password: None,
        });
        let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
        let mut saved = None;
        let unlocked = AtomicBool::new(true);

        finish_sync(
            &mut session,
            &mut saved,
            Synced {
                vault,
                kind: SyncKind::AfterUnlock,
                result: Err(CoreError::Rejected("the token was revoked".to_string())),
                elapsed: Duration::ZERO,
            },
            &events,
            &unlocked,
        )
        .await;

        assert!(session.is_none());
        assert!(!unlocked.load(Ordering::SeqCst));
        assert!(matches!(received.try_recv(), Ok(Event::Locked)));
    }

    #[tokio::test]
    async fn a_sync_for_a_replaced_vault_is_ignored() {
        let vault = Arc::new(Vault::connect(None).unwrap());
        let replaced = Arc::new(Vault::connect(None).unwrap());
        let mut session = Some(Session {
            vault: Arc::clone(&vault),
            email: "test@example.com".to_string(),
            password: None,
        });
        let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
        let mut saved = None;

        finish_sync(
            &mut session,
            &mut saved,
            Synced {
                vault: replaced,
                kind: SyncKind::AfterUnlock,
                result: Err(CoreError::Rejected("the token was revoked".to_string())),
                elapsed: Duration::ZERO,
            },
            &events,
            &AtomicBool::new(true),
        )
        .await;

        assert!(session.is_some());
        assert!(received.try_recv().is_err());
    }
}
