//! Account form state and request lifecycle, independent of window APIs.
use super::account::{self, Login, Session};
use anyhow::Result;
use std::sync::{mpsc, Arc};

pub enum Completion {
    Login(Result<Arc<Session>>),
    Logout,
    Disconnected,
}

/// Persistence policy is common; protection and file IO belong to the store.
pub trait SessionStore {
    fn save(&self, email: &str, token: &str) -> Result<()>;
    fn clear(&self, forget_email: bool) -> Result<()>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersistenceNotice {
    PreviousSignInNotRemoved,
    ProtectedSessionNotSaved,
}
impl PersistenceNotice {
    pub fn text(self) -> &'static str {
        match self {
            Self::PreviousSignInNotRemoved => "The old saved sign-in could not be removed. Check write access to settings.json.",
            Self::ProtectedSessionNotSaved => "Signed in, but the protected session could not be saved. This sign-in will stay in memory.",
        }
    }
}

// No Debug/Serialize: account details and session tokens must not enter logs.
pub struct SignIn {
    pub session: Option<Arc<Session>>,
    pub email: String,
    pub remember_login: bool,
    incoming: Option<mpsc::Receiver<Completion>>,
    message: String,
}
impl SignIn {
    pub fn new(session: Option<Arc<Session>>, email: String, remember_login: bool) -> Self {
        Self {
            session,
            email,
            remember_login,
            incoming: None,
            message: String::new(),
        }
    }
    pub fn busy(&self) -> bool {
        self.incoming.is_some()
    }
    pub fn message(&self) -> &str {
        &self.message
    }
    fn begin(&mut self, message: &str) -> Option<mpsc::Sender<Completion>> {
        if self.busy() {
            return None;
        }
        let (tx, rx) = mpsc::channel();
        self.incoming = Some(rx);
        self.message = message.into();
        Some(tx)
    }
    pub fn login(&mut self, login: Login, remember: bool) -> Result<bool> {
        if self.busy() {
            return Ok(false);
        }
        if login.email.trim().is_empty() || login.password.is_empty() {
            self.message = "Email and password are required.".into();
            anyhow::bail!(self.message.clone());
        }
        let tx = self.begin("Signing in securely...").unwrap();
        self.email = login.email.trim().into();
        self.remember_login = remember;
        // Password and optional 2FA code are owned only by this request and
        // zeroized on completion. No duplicate form copy or retry cache.
        std::thread::spawn(move || {
            let result = account::login(&login).map(Arc::new);
            if let Err(mpsc::SendError(Completion::Login(Ok(session)))) =
                tx.send(Completion::Login(result))
            {
                session.revoke();
            }
        });
        Ok(true)
    }
    pub fn accept(
        &mut self,
        session: Arc<Session>,
        store: &impl SessionStore,
        notice: impl FnOnce(PersistenceNotice),
    ) {
        if self.remember_login {
            match store.save(&self.email, session.token()) {
                Ok(()) => session.remembered(),
                Err(_) => {
                    notice(PersistenceNotice::ProtectedSessionNotSaved);
                    self.remember_login = false;
                }
            }
        } else if store.clear(false).is_err() {
            notice(PersistenceNotice::PreviousSignInNotRemoved);
        }
        // Keep notice delivery before committing the session to the view:
        // native modal feedback previously ran at this exact point.
        self.session = Some(session);
    }
    pub fn prepare_logout(&mut self, store: &impl SessionStore) -> Result<()> {
        if store.clear(true).is_err() {
            self.message =
                "Could not remove the saved sign-in. Check write access to settings.json.".into();
            anyhow::bail!(self.message.clone());
        }
        self.remember_login = false;
        Ok(())
    }
    pub fn logout(&mut self) -> bool {
        if self.busy() || self.session.is_none() {
            return false;
        }
        let tx = self.begin("Signing out...").unwrap();
        let session = self.session.take().unwrap();
        self.remember_login = false;
        std::thread::spawn(move || {
            session.revoke();
            let _ = tx.send(Completion::Logout);
        });
        true
    }
    pub fn poll(&mut self) -> Option<Completion> {
        let completion = match self.incoming.as_ref()?.try_recv() {
            Ok(completion) => completion,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Completion::Disconnected,
        };
        self.incoming = None;
        self.message = match &completion {
            Completion::Login(Ok(_)) => String::new(),
            Completion::Login(Err(error)) => error.to_string(),
            Completion::Logout => "Signed out. Enter your account details to sign in.".into(),
            Completion::Disconnected => {
                "The account request ended unexpectedly. Please retry.".into()
            }
        };
        Some(completion)
    }
    pub fn expired(&mut self) {
        self.session = None;
        self.remember_login = false;
    }
    pub fn expire_saved(&mut self, store: &impl SessionStore) {
        // Preserve the existing expiration policy: rejection always clears the
        // live session, even if the old protected file cannot be updated.
        let _ = store.clear(false);
        self.expired();
    }
    pub fn close(&mut self, has_saved_session: impl FnOnce() -> bool) {
        if let Some(session) = self.session.take() {
            if !has_saved_session() {
                session.revoke();
            }
        }
    }
    pub fn cancel(&mut self) {
        // Closing the view drops the receiver. A late successful sign-in is
        // revoked by its worker instead of silently saving an unwanted token.
        self.incoming = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    #[derive(Default)]
    struct Store {
        fail: bool,
        saved: RefCell<Option<(String, String)>>,
        cleared: RefCell<Vec<bool>>,
    }
    impl SessionStore for Store {
        fn save(&self, email: &str, token: &str) -> Result<()> {
            if self.fail {
                anyhow::bail!("offline-storage-failure");
            }
            *self.saved.borrow_mut() = Some((email.into(), token.into()));
            Ok(())
        }
        fn clear(&self, forget_email: bool) -> Result<()> {
            self.cleared.borrow_mut().push(forget_email);
            if self.fail {
                anyhow::bail!("offline-storage-failure");
            }
            Ok(())
        }
    }
    fn form() -> SignIn {
        SignIn::new(None, "saved@example.invalid".into(), true)
    }
    #[test]
    fn requests_are_single_flight_and_errors_allow_retry_without_window_apis() {
        let mut form = form();
        let tx = form.begin("Signing in securely...").unwrap();
        assert!(form.busy());
        assert!(form.begin("Must not replace request").is_none());
        assert_eq!(form.message(), "Signing in securely...");
        assert!(form.poll().is_none());
        assert!(tx
            .send(Completion::Login(Err(anyhow::anyhow!(
                "Check your credentials and two-factor code."
            ))))
            .is_ok());
        assert!(matches!(form.poll(), Some(Completion::Login(Err(_)))));
        assert!(!form.busy());
        assert!(form.message().contains("two-factor"));
        assert_eq!(form.email, "saved@example.invalid");
        let retry = form.begin("Signing in securely...").unwrap();
        drop(retry);
        assert!(matches!(form.poll(), Some(Completion::Disconnected)));
        assert!(!form.busy());
        assert!(form.message().contains("Please retry"));
    }
    #[test]
    fn cancellation_and_expiration_do_not_restore_old_sessions() {
        let mut form = form();
        let tx = form.begin("Signing in securely...").unwrap();
        form.cancel();
        assert!(!form.busy());
        let session = Arc::new(Session::from_token("offline-fixture").unwrap());
        assert!(tx.send(Completion::Login(Ok(session.clone()))).is_err());
        assert!(form.poll().is_none());
        form.session = Some(session);
        form.expired();
        assert!(form.session.is_none());
        assert!(!form.remember_login);
        assert!(!form.logout()); // Never contacts a server in this fixture.
        let tx = form.begin("Signing out...").unwrap();
        assert!(tx.send(Completion::Logout).is_ok());
        assert!(matches!(form.poll(), Some(Completion::Logout)));
        assert!(form.message().starts_with("Signed out."));
    }
    #[test]
    fn successful_reply_retains_session_identity_until_existing_storage_policy_applies() {
        let session = Arc::new(Session::from_token("offline-fixture").unwrap());
        let mut form = SignIn::new(None, "user@example.invalid".into(), false);
        let tx = form.begin("Signing in securely...").unwrap();
        assert!(tx.send(Completion::Login(Ok(session.clone()))).is_ok());
        let Some(Completion::Login(Ok(reply))) = form.poll() else {
            panic!("Expected successful offline reply");
        };
        assert!(!form.busy());
        assert!(form.message().is_empty());
        assert!(Arc::ptr_eq(&reply, &session));
        // Only explicit acceptance applies persistence policy; polling alone
        // must not save or mark a session remembered.
        assert!(!form.remember_login);
        assert!(form.session.is_none());
        let store = Store::default();
        form.accept(reply, &store, |_| panic!("No notice expected"));
        assert_eq!(&*store.cleared.borrow(), &[false]);
        assert!(Arc::ptr_eq(&form.session.take().unwrap(), &session));
    }
    #[test]
    fn accepted_sessions_keep_the_previous_remember_and_failure_policy() {
        for remember in [false, true] {
            for fail in [false, true] {
                let store = Store {
                    fail,
                    ..Default::default()
                };
                let session = Arc::new(Session::from_token("offline-session-fixture").unwrap());
                let mut form = SignIn::new(None, "user@example.invalid".into(), remember);
                let mut notices = Vec::new();
                form.accept(session.clone(), &store, |notice| notices.push(notice));
                assert!(Arc::ptr_eq(form.session.as_ref().unwrap(), &session));
                assert_eq!(form.remember_login, remember && !fail);
                assert_eq!(
                    &*store.cleared.borrow(),
                    if remember { &[][..] } else { &[false][..] }
                );
                assert_eq!(store.saved.borrow().is_some(), remember && !fail);
                if remember && !fail {
                    assert_eq!(
                        store.saved.borrow().as_ref().unwrap(),
                        &(
                            "user@example.invalid".to_owned(),
                            "offline-session-fixture".to_owned()
                        )
                    );
                }
                if fail {
                    assert_eq!(
                        notices,
                        [if remember {
                            PersistenceNotice::ProtectedSessionNotSaved
                        } else {
                            PersistenceNotice::PreviousSignInNotRemoved
                        }]
                    );
                    assert!(!notices[0].text().contains("offline-session-fixture"));
                    assert!(!notices[0].text().contains("offline-storage-failure"));
                } else {
                    assert!(notices.is_empty());
                }
            }
        }
    }
    #[test]
    fn logout_and_expiration_have_shared_storage_failure_rules() {
        let session = Arc::new(Session::from_token("offline-session-fixture").unwrap());
        for fail in [false, true] {
            let store = Store {
                fail,
                ..Default::default()
            };
            let mut form = SignIn::new(Some(session.clone()), "user@example.invalid".into(), true);
            assert_eq!(form.prepare_logout(&store).is_err(), fail);
            assert!(form.session.is_some()); // Explicit revocation still starts only in logout().
            assert_eq!(form.remember_login, fail);
            assert_eq!(&*store.cleared.borrow(), &[true]);
            if fail {
                assert_eq!(
                    form.message(),
                    "Could not remove the saved sign-in. Check write access to settings.json."
                );
            }
            form.expire_saved(&store);
            assert!(form.session.is_none());
            assert!(!form.remember_login);
            assert_eq!(&*store.cleared.borrow(), &[true, false]);
        }
    }
    #[test]
    fn close_reads_storage_only_when_the_form_still_owns_a_session() {
        let mut form = form();
        form.close(|| panic!("An empty form must not query storage"));
        let session = Arc::new(Session::from_token("offline-fixture").unwrap());
        form.session = Some(session.clone());
        let mut queried = false;
        form.close(|| {
            queried = true;
            true
        });
        assert!(queried);
        assert!(form.session.is_none());
        assert_eq!(Arc::strong_count(&session), 1);
    }
    #[test]
    fn missing_credentials_leave_no_pending_request_or_saved_secrets() {
        let mut form = form();
        for (email, password, tfa) in [(" ", "fixture", "012345"), ("a@b.invalid", "", "")] {
            let result = form.login(
                Login {
                    email: email.into(),
                    password: zeroize::Zeroizing::new(password.into()),
                    tfa: zeroize::Zeroizing::new(tfa.into()),
                },
                false,
            );
            assert!(result.is_err());
            assert!(!form.busy());
            assert_eq!(form.message(), "Email and password are required.");
            assert_eq!(form.email, "saved@example.invalid");
            assert!(form.remember_login);
        }
    }
}
