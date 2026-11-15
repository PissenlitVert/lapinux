//! Connexion Steam et authentification.

use super::*;

impl Shared {
    /// Démarre la connexion Steam en arrière-plan avec Lapinux (mot de passe,
    /// Steam Guard, jeton de refresh). Rendre la main tout de suite permet
    /// d'attendre un code Steam Guard sans bloquer l'interface ; l'état est
    /// relu par `SteamStatus`.
    pub fn steam_login(
        self: &Arc<Self>,
        username: &str,
        password: &str,
        guard: Option<&str>,
        token: Option<&str>,
    ) -> Result<Envelope> {
        let username = username.trim().to_string();
        if username.is_empty() {
            return Err(lapinux_core::Error::Other(
                "renseignez votre compte Steam".into(),
            ));
        }
        {
            let mut state = self.login.lock().map_err(|_| {
                lapinux_core::Error::Other("état de connexion Steam indisponible".into())
            })?;
            if state.running {
                return Err(lapinux_core::Error::Other(
                    "une connexion Steam est déjà en cours".into(),
                ));
            }
            *state = SteamLoginState {
                running: true,
                stage: "connexion".into(),
                ..Default::default()
            };
        }
        self.login_cancel.store(false, Ordering::SeqCst);
        if let Ok(mut slot) = self.login_guard.lock() {
            *slot = None;
        }

        let shared = Arc::clone(self);
        let password = password.to_string();
        let guard = guard.map(str::to_string);
        let token = token
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(str::to_string);
        std::thread::spawn(move || shared.run_steam_login(username, password, guard, token));

        Ok(Envelope::ok(&serde_json::json!({ "pending": true })))
    }

    /// Connexion par QR code : aucune saisie de mot de passe, l'application
    /// Steam du téléphone approuve le défi. L'URL est publiée dans l'état dès
    /// que Steam la rend, l'interface n'a qu'à afficher le QR.
    pub fn steam_login_qr(self: &Arc<Self>) -> Result<Envelope> {
        {
            let mut state = self.login.lock().map_err(|_| {
                lapinux_core::Error::Other("état de connexion Steam indisponible".into())
            })?;
            if state.running {
                return Err(lapinux_core::Error::Other(
                    "une connexion Steam est déjà en cours".into(),
                ));
            }
            *state = SteamLoginState {
                running: true,
                stage: "connexion par QR".into(),
                ..Default::default()
            };
        }
        self.login_cancel.store(false, Ordering::SeqCst);
        if let Ok(mut slot) = self.login_guard.lock() {
            *slot = None;
        }
        let shared = Arc::clone(self);
        std::thread::spawn(move || shared.run_qr_login());
        Ok(Envelope::ok(&serde_json::json!({ "pending": true })))
    }

    /// Enregistre le jeton dès l'approbation du téléphone. Si le logon CM
    /// échoue ensuite (rate-limit, coupure), le scan n'est pas perdu : le jeton
    /// reste utilisable par le runtime Steam et les lectures Web.
    fn remember_qr_token(&self, tokens: &cm::auth::Tokens) {
        let name = if tokens.account_name.trim().is_empty() {
            match session::load_from(&self.session_path()) {
                Some(saved) if saved.steam_id == tokens.steam_id => saved.account_name,
                _ => {
                    tracing::warn!(
                        "jeton QR approuvé pour steamid {} sans nom de compte ; non enregistré",
                        tokens.steam_id
                    );
                    return;
                }
            }
        } else {
            tokens.account_name.clone()
        };
        let saved = SavedSession {
            account_name: name.clone(),
            steam_id: tokens.steam_id,
            refresh_token: tokens.refresh_token.clone(),
            guard_data: tokens.guard_data.clone(),
            saved_at: None,
        };
        match session::save_to(&self.session_path(), &saved) {
            Ok(()) => tracing::info!(
                "jeton QR enregistré pour {name} (steamid {}), logon CM en cours",
                tokens.steam_id
            ),
            Err(error) => tracing::warn!("jeton QR non enregistré : {error}"),
        }
    }

    /// Défi QR puis attente de l'approbation ; le logon suit sur la même
    /// connexion, comme pour la connexion par mot de passe.
    fn run_qr_login(self: Arc<Self>) {
        tracing::info!("connexion Steam par QR demandée");
        let config = cm::Config::from_env();
        let mut on_url = |url: &str| {
            self.update_login(|state| {
                state.qr = Some(url.to_string());
                state.stage = "QR à scanner".into();
            });
        };
        let cancelled = || self.login_aborted();
        let mut on_tokens = |tokens: &cm::auth::Tokens| self.remember_qr_token(tokens);
        match cm::sign_in_qr_with_tokens(&config, &mut on_url, &cancelled, &mut on_tokens) {
            Ok(session) => {
                if self.login_aborted() {
                    return;
                }
                let mut name = session.account_name.clone().unwrap_or_default();
                if name.trim().is_empty() {
                    // Le poll QR omet parfois le nom : le jeton, lui, porte le
                    // SteamID, qui suffit à retrouver le compte déjà connu.
                    name = session::load_from(&self.session_path())
                        .filter(|saved| saved.steam_id == session.steam_id)
                        .map(|saved| saved.account_name)
                        .unwrap_or_default();
                }
                if name.trim().is_empty() {
                    self.finish_login(SteamLoginState {
                        running: false,
                        stage: "erreur".into(),
                        error: Some("QR approuvé mais Steam n'a pas rendu le compte".into()),
                        ..Default::default()
                    });
                    return;
                }
                self.finish_steam_session(&name, &session);
            }
            Err(error) => {
                if self.login_aborted() {
                    return;
                }
                tracing::warn!("connexion Steam par QR impossible : {error}");
                self.finish_login(SteamLoginState {
                    running: false,
                    stage: "erreur".into(),
                    error: Some(message_utilisateur(&error)),
                    ..Default::default()
                });
            }
        }
    }

    /// Annule la connexion en cours : le fil réseau s'arrête dès qu'il rend la
    /// main, l'interface est libérée tout de suite et l'état n'est plus écrasé
    /// par une tentative tardive.
    pub fn steam_login_cancel(&self) -> Result<Envelope> {
        let running = self
            .login
            .lock()
            .map(|state| state.running)
            .unwrap_or(false);
        if !running {
            return Err(lapinux_core::Error::Other(
                "aucune connexion Steam n'est en cours".into(),
            ));
        }
        self.login_cancel.store(true, Ordering::SeqCst);
        if let Ok(mut slot) = self.login_guard.lock() {
            *slot = None;
        }
        self.finish_login(SteamLoginState {
            stage: "annulé".into(),
            message: Some("connexion Steam annulée".into()),
            ..Default::default()
        });
        Ok(Envelope::ok(&self.steam_status()))
    }

    fn login_aborted(&self) -> bool {
        self.login_cancel.load(Ordering::SeqCst)
    }

    /// Transmet le code Steam Guard demandé par la connexion en cours.
    pub fn steam_login_guard(&self, code: &str) -> Result<Envelope> {
        let code = code.trim();
        if code.is_empty() {
            return Err(lapinux_core::Error::Other(
                "saisissez le code Steam Guard".into(),
            ));
        }
        let sender = self
            .login_guard
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        let Some(sender) = sender else {
            return Err(lapinux_core::Error::Other(
                "aucun code Steam Guard n'est attendu".into(),
            ));
        };
        sender
            .send(code.to_string())
            .map_err(|_| lapinux_core::Error::Other("la connexion Steam a expiré".into()))?;
        // L'état est mis à jour par la connexion elle-même : un code faux peut
        // redemander immédiatement un nouveau code, et l'interface doit voir
        // cette demande plutôt qu'un état « vérification » écrasé après coup.
        Ok(Envelope::unit())
    }

    fn run_steam_login(
        self: Arc<Self>,
        username: String,
        password: String,
        guard: Option<String>,
        token: Option<String>,
    ) {
        tracing::info!("connexion Steam demandée pour {username}");
        let mut initial_guard = guard;
        let mut failures = Vec::new();
        let mut password_error = None;

        // 1. Jeton Lapinux collé dans l'écran de connexion : il remplace le mot
        // de passe et sert de repli à la session enregistrée sur le disque.
        if let Some(token) = token.as_deref() {
            self.update_login(|state| state.stage = "connexion par jeton Lapinux".into());
            match self.sign_in_with_token(&username, token) {
                Ok(session) => {
                    if self.login_aborted() {
                        return;
                    }
                    self.finish_steam_session(&username, &session);
                    return;
                }
                Err(error) => failures.push(format!("jeton fourni : {error}")),
            }
            if self.login_aborted() {
                return;
            }
        }

        // 2. Jeton Lapinux déjà enregistré : reconnexion sans mot de passe.
        if let Some(saved) = self.saved_session(&username) {
            self.update_login(|state| state.stage = "jeton enregistré".into());
            match self.sign_in_with_token(&username, &saved.refresh_token) {
                Ok(session) => {
                    if self.login_aborted() {
                        return;
                    }
                    self.finish_steam_session(&username, &session);
                    return;
                }
                Err(error) => failures.push(format!("jeton Lapinux : {error}")),
            }
            if self.login_aborted() {
                return;
            }
        }

        // 3. Identifiants classiques : le service Authentication rend un jeton.
        if !password.is_empty() {
            let config = cm::Config::from_env();
            self.update_login(|state| state.stage = "connexion Lapinux".into());
            let guard_data = self
                .saved_session(&username)
                .and_then(|saved| saved.guard_data);
            let auth = cm::Auth::Account {
                name: username.clone(),
                password: password.clone(),
                guard_data,
            };
            let mut prompt = |kind, retry| self.steam_guard_prompt(kind, retry, &mut initial_guard);
            let result = cm::sign_in(&config, &auth, &mut prompt);
            match result {
                Ok(session) => {
                    if self.login_aborted() {
                        return;
                    }
                    self.finish_steam_session(&username, &session);
                    return;
                }
                Err(error) => password_error = Some(message_utilisateur(&error)),
            }
        }
        if self.login_aborted() {
            return;
        }
        // L'écran montre la tentative qui compte : un jeton enregistré refusé
        // ne doit pas enterrer l'erreur du mot de passe saisi, et inversement
        // quand aucun mot de passe n'a été fourni.
        let error = match password_error {
            Some(error) => error,
            None if !failures.is_empty() => failures.join(" · "),
            None => "renseignez votre mot de passe Steam pour vous connecter".into(),
        };
        tracing::warn!("connexion Steam impossible pour {username} : {error}");
        self.finish_login(SteamLoginState {
            running: false,
            stage: "erreur".into(),
            error: Some(error),
            ..Default::default()
        });
    }

    /// Reconnexion sans mot de passe par un jeton de refresh Lapinux. Une
    /// connexion par jeton ne déclenche pas Steam Guard : si le serveur en
    /// demande un, le jeton est refusé et l'appelant essaye autre chose.
    pub(super) fn sign_in_with_token(
        &self,
        username: &str,
        refresh_token: &str,
    ) -> std::result::Result<cm::Session, cm::Error> {
        let config = cm::Config::from_env();
        let auth = cm::Auth::Token {
            name: username.to_string(),
            refresh_token: refresh_token.to_string(),
        };
        let mut prompt = |_, _| Err(cm::Error::Auth("aucun code Steam Guard attendu".into()));
        match cm::sign_in(&config, &auth, &mut prompt) {
            // Un jeton qui échoue n'est pas forcément mort : Steam répond
            // parfois « mot de passe invalide » quand une autre session du
            // compte vient de s'ouvrir. Le journal garde la réponse exacte et
            // l'appelant passe au mot de passe.
            Err(error) => {
                tracing::warn!("connexion par jeton Lapinux refusée : {error}");
                Err(error)
            }
            Ok(session) => {
                // Steam fait tourner le jeton de refresh quand on ouvre une
                // session : la version rendue remplace celle du disque, sinon
                // l'ancienne meurt à la rotation suivante et le compte finit
                // par refuser toute connexion (« mot de passe invalide »).
                if let Some(renewed) = session
                    .refresh_token
                    .as_deref()
                    .filter(|renewed| !renewed.trim().is_empty() && *renewed != refresh_token)
                {
                    let saved = SavedSession {
                        account_name: username.to_string(),
                        steam_id: session.steam_id,
                        refresh_token: renewed.to_string(),
                        guard_data: session.guard_data.clone(),
                        saved_at: None,
                    };
                    match session::save_to(&self.session_path(), &saved) {
                        Ok(_) => tracing::info!("jeton Steam tourné par la session, enregistré"),
                        Err(error) => tracing::warn!("jeton Steam tourné non enregistré : {error}"),
                    }
                }
                Ok(session)
            }
        }
    }

    /// Demande le code Steam Guard à l'interface et attend sa réponse. Le
    /// premier code fourni dans le formulaire est consommé sans attente ; les
    /// suivants arrivent par `SteamLoginGuard`.
    pub(super) fn steam_guard_prompt(
        &self,
        kind: cm::Guard,
        retry: bool,
        initial: &mut Option<String>,
    ) -> std::result::Result<String, cm::Error> {
        let (guard, stage, message) = match kind {
            cm::Guard::EmailCode => (
                SteamGuardKind::EmailCode,
                "code Steam Guard (e-mail)",
                "Saisissez le code envoyé à l'adresse e-mail du compte.",
            ),
            cm::Guard::TwoFactor => (
                SteamGuardKind::TwoFactor,
                "code Steam Guard (application)",
                "Saisissez le code affiché par l'application Steam.",
            ),
            cm::Guard::DeviceConfirmation => (
                SteamGuardKind::DeviceConfirmation,
                "confirmation dans l'application Steam",
                "Confirmez la connexion dans l'application Steam ; l'écran attend la réponse.",
            ),
            cm::Guard::EmailConfirmation => (
                SteamGuardKind::EmailConfirmation,
                "confirmation par e-mail",
                "Confirmez la connexion via le lien envoyé par e-mail ; l'écran attend la réponse.",
            ),
        };
        let wants_code = matches!(kind, cm::Guard::EmailCode | cm::Guard::TwoFactor);
        let using_initial = wants_code && initial.is_some();
        // Le canal est installé avant de publier l'état : dès que l'interface
        // voit la demande, elle peut envoyer le code sans course.
        let receiver = if wants_code && !using_initial {
            let (sender, receiver) = mpsc::channel();
            if let Ok(mut slot) = self.login_guard.lock() {
                *slot = Some(sender);
            }
            Some(receiver)
        } else {
            None
        };

        self.update_login(|state| {
            state.stage = if using_initial {
                "vérification du code Steam Guard".into()
            } else {
                stage.into()
            };
            state.guard = if using_initial { None } else { Some(guard) };
            state.guard_message = Some(message.into());
            state.guard_retry = retry;
        });

        if wants_code {
            if let Some(code) = initial.take() {
                return Ok(code);
            }
            let Some(receiver) = receiver else {
                return Err(cm::Error::Auth("code Steam Guard indisponible".into()));
            };
            // Attente par tranches : une annulation doit rendre la main sans
            // attendre les trois minutes du délai Steam Guard.
            let deadline = Instant::now() + Duration::from_secs(180);
            let result = loop {
                if self.login_aborted() {
                    break Err(cm::Error::Auth("connexion annulée".into()));
                }
                match receiver.recv_timeout(Duration::from_millis(200)) {
                    Ok(code) => break Ok(code),
                    Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => continue,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        break Err(cm::Error::Auth("délai Steam Guard dépassé".into()))
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        break Err(cm::Error::Auth("connexion annulée".into()))
                    }
                }
            };
            if let Ok(mut slot) = self.login_guard.lock() {
                *slot = None;
            }
            if result.is_ok() {
                self.update_login(|state| {
                    state.guard = None;
                    state.guard_retry = false;
                    state.stage = "vérification du code".into();
                });
            }
            return result;
        }
        Ok(String::new())
    }

    fn finish_steam_session(self: &Arc<Self>, username: &str, session: &cm::Session) {
        // Une annulation pendant le réseau ne doit ni enregistrer de jeton ni
        // écraser l'état « annulé » déjà affiché.
        if self.login_aborted() {
            return;
        }
        let mut notes = Vec::new();
        if let Some(refresh) = session.refresh_token.as_deref() {
            let saved = SavedSession {
                account_name: username.to_string(),
                steam_id: session.steam_id,
                refresh_token: refresh.to_string(),
                guard_data: session.guard_data.clone(),
                saved_at: None,
            };
            if let Err(error) = session::save_to(&self.session_path(), &saved) {
                notes.push(format!("jeton non enregistré : {error}"));
            }
        }
        let mut config = self.config_snapshot();
        config.steam.username = username.to_string();
        let _ = self.save_config(config);
        // Le jeton qui vient d'être obtenu est valide par construction ; la
        // vérification n'a pas à retéléphoner à Steam.
        self.mark_token_valid();

        tracing::info!(
            "connecté via Lapinux ({username}, steamid {})",
            session.steam_id
        );
        let message = if notes.is_empty() {
            "connecté via Lapinux ; jeton réutilisable".to_string()
        } else {
            format!("connecté via Lapinux ; {}", notes.join(" ; "))
        };
        self.finish_login(SteamLoginState {
            running: false,
            connected: true,
            backend: Some("Lapinux".into()),
            stage: "terminé".into(),
            message: Some(message),
            ..Default::default()
        });
        // Le jeton vient d'être remplacé : la session sociale repart avec lui,
        // et le compte devient visible de ses amis.
        self.restart_social_in_background();
    }
}
