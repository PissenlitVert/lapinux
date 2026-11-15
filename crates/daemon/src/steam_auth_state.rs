//! État du compte et du jeton Steam.

use super::*;

impl Shared {
    pub fn steam_status(&self) -> SteamStatus {
        let config = self.config_snapshot();
        let login = self
            .login
            .lock()
            .map(|state| state.clone())
            .unwrap_or_default();
        let saved = session::load_from(&self.session_path());
        let token_saved = saved
            .as_ref()
            .map(|saved| !saved.refresh_token.trim().is_empty())
            .unwrap_or(false);
        // Les jetons écrits avant l'horodatage n'en ont pas : la date du
        // fichier dit alors à peu près quand il a été obtenu.
        let stamp = saved
            .as_ref()
            .and_then(|saved| saved.saved_at.or_else(|| file_stamp(&self.session_path())));
        let check = self.token_check_for(stamp);
        let token = saved.as_ref().map(|saved| SteamTokenState {
            account_name: saved.account_name.clone(),
            saved_at: stamp,
            // Un jeton absent ne peut pas être valide, même sans vérification.
            verdict: if saved.refresh_token.trim().is_empty() {
                SteamTokenVerdict::Expired
            } else {
                check.verdict
            },
            checked_at: check.checked_at,
            detail: check.detail.clone(),
        });
        // Un jeton expiré ou temporairement bloqué ne peut pas ouvrir de
        // session : l'écran ne doit pas afficher « connecté », même si la
        // session du démarrage avait abouti.
        let unavailable = token
            .as_ref()
            .map(|token| {
                matches!(
                    token.verdict,
                    SteamTokenVerdict::Expired | SteamTokenVerdict::RateLimited
                )
            })
            .unwrap_or(false);
        // L'identifiant sert à ouvrir le profil public : la session le garde
        // d'une connexion à l'autre, même sans jeton.
        let steam_id = saved
            .map(|saved| saved.steam_id)
            .filter(|steam_id| *steam_id > 0);
        SteamStatus {
            username: config.steam.username.clone(),
            connected: !unavailable && (login.connected || token_saved),
            token_saved,
            token,
            steam_id,
            library_root: config.library_root(),
            login,
            default_proton: lapinux_proton::recommended(&self.protons_cached())
                .map(|proton| proton.name.clone()),
            default_language: lapinux_steam::native::default_language_label(),
        }
    }

    /// Verdict applicable au jeton enregistré. Un verdict rendu pour un autre
    /// jeton ne vaut plus rien : le fichier a été remplacé ou réécrit.
    fn token_check_for(&self, stamp: Option<i64>) -> TokenCheck {
        let check = self
            .token_check
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default();
        if stamp.is_none() || check.token_saved_at != stamp {
            return TokenCheck::default();
        }
        check
    }

    /// Vérifie le jeton enregistré auprès de Steam, en arrière-plan : le logon
    /// par jeton dure une seconde et ne doit pas retenir l'interface.
    ///
    /// Une seule vérification à la fois ; un verdict déjà connu (valide ou
    /// jeton refusé) n'est pas redemandé, un échec réseau ne condamne pas le
    /// jeton.
    pub fn steam_token_check(self: &Arc<Self>) -> Result<Envelope> {
        let saved = session::load_from(&self.session_path())
            .filter(|saved| !saved.refresh_token.trim().is_empty());
        let Some(saved) = saved else {
            return Ok(Envelope::ok(&self.steam_status()));
        };
        let stamp = saved.saved_at.or_else(|| file_stamp(&self.session_path()));
        // Le verrou est relâché avant toute lecture d'état : `steam_status`
        // relit le même verrou, et un return sous garde le bloquerait pour de bon.
        let start = {
            let mut check = self
                .token_check
                .lock()
                .map_err(|_| lapinux_core::Error::Other("état du jeton indisponible".into()))?;
            let start = needs_check(&check, stamp);
            if start {
                *check = TokenCheck {
                    verdict: SteamTokenVerdict::Checking,
                    running: true,
                    token_saved_at: stamp,
                    ..Default::default()
                };
            }
            start
        };
        if !start {
            return Ok(Envelope::ok(&self.steam_status()));
        }

        let shared = Arc::clone(self);
        std::thread::spawn(move || {
            // Première tentative par la session partagée : si le jeton est bon,
            // la vérification réutilise (ou ouvre) la connexion que toutes les
            // lectures emploieront — aucun logon de plus, donc aucune session
            // éjectée. C'est le cas courant.
            let verdict = match lapinux_steam::shared::verify_blocking(
                &saved.account_name,
                &saved.refresh_token,
            ) {
                Ok(()) => {
                    tracing::info!(
                        "jeton Lapinux vérifié : valide (steamid {})",
                        saved.steam_id
                    );
                    (SteamTokenVerdict::Valid, None)
                }
                Err(error) if error.is_rate_limited() => {
                    let message = message_utilisateur(&cm::Error::Auth(error.to_string()));
                    tracing::info!("jeton Lapinux non vérifié : {message}");
                    (SteamTokenVerdict::RateLimited, Some(message))
                }
                Err(error) => {
                    // La session partagée ne dit pas pourquoi elle a échoué :
                    // `cm::sign_in` tranche entre un jeton mort et une panne
                    // réseau, et c'est le seul cas où une connexion de plus se
                    // justifie.
                    tracing::debug!("session partagée : {error}");
                    let config = cm::Config::from_env();
                    let auth = cm::Auth::Token {
                        name: saved.account_name.clone(),
                        refresh_token: saved.refresh_token.clone(),
                    };
                    let mut prompt =
                        |_, _| Err(cm::Error::Auth("aucun code Steam Guard attendu".into()));
                    match cm::sign_in(&config, &auth, &mut prompt) {
                        Ok(session) => {
                            tracing::info!(
                                "jeton Lapinux vérifié : valide (steamid {})",
                                session.steam_id
                            );
                            (SteamTokenVerdict::Valid, None)
                        }
                        // Un refus d'authentification condamne le jeton ; une
                        // panne réseau ou un CM muet ne dit rien de lui.
                        Err(error @ cm::Error::Auth(_)) => {
                            let message = message_utilisateur(&error);
                            if error.is_rate_limited() {
                                tracing::info!("jeton Lapinux limité ({message})");
                                (SteamTokenVerdict::RateLimited, Some(message))
                            } else {
                                tracing::info!("jeton Lapinux vérifié : refusé ({message})");
                                (SteamTokenVerdict::Expired, Some(message))
                            }
                        }
                        Err(other) => {
                            let message = message_utilisateur(&other);
                            tracing::info!("jeton Lapinux non vérifié : {message}");
                            (SteamTokenVerdict::Unknown, Some(message))
                        }
                    }
                }
            };
            if let Ok(mut check) = shared.token_check.lock() {
                *check = TokenCheck {
                    verdict: verdict.0,
                    checked_at: Some(session::now_unix()),
                    detail: verdict.1,
                    running: false,
                    token_saved_at: stamp,
                };
            }
        });
        Ok(Envelope::ok(&self.steam_status()))
    }

    /// Le jeton vient d'être obtenu par une connexion : il est valide.
    pub(super) fn mark_token_valid(&self) {
        // La date du jeton écrit décide de la validité de ce verdict.
        let stamp = session::load_from(&self.session_path())
            .and_then(|saved| saved.saved_at)
            .or_else(|| file_stamp(&self.session_path()));
        if let Ok(mut check) = self.token_check.lock() {
            *check = TokenCheck {
                verdict: SteamTokenVerdict::Valid,
                checked_at: Some(session::now_unix()),
                detail: None,
                running: false,
                token_saved_at: stamp,
            };
        }
    }

    /// Oublie le jeton Lapinux enregistré ; aucun mot de passe n'est conservé.
    pub fn steam_forget_token(&self) -> Result<Envelope> {
        if self
            .login
            .lock()
            .map(|state| state.running)
            .unwrap_or(false)
        {
            return Err(lapinux_core::Error::Other(
                "attendez la fin de la connexion Steam en cours".into(),
            ));
        }
        let path = self.session_path();
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(lapinux_core::Error::Other(format!(
                    "jeton non supprimé ({}): {error}",
                    path.display()
                )))
            }
        }
        // Plus de jeton : plus de verdict à afficher.
        if let Ok(mut check) = self.token_check.lock() {
            *check = TokenCheck::default();
        }
        // La session sociale vivait de ce jeton : elle s'arrête avec lui.
        self.stop_social();
        // L'état « connecté » ne survit pas au jeton : `steam_status`
        // recompose la réponse à partir de la session disque.
        self.finish_login(SteamLoginState {
            stage: "jeton oublié".into(),
            ..Default::default()
        });
        Ok(Envelope::ok(&self.steam_status()))
    }

    /// Déconnecte le compte : jeton, identité et session sociale. C'est le
    /// pendant Steam du bouton « Déconnecter » des autres boutiques ;
    /// « oublier le jeton » garde, lui, le nom du compte pour se reconnecter.
    pub fn steam_disconnect(&self) -> Result<Envelope> {
        let _ = self.steam_forget_token()?;
        let mut config = self.config_snapshot();
        if !config.steam.username.trim().is_empty() {
            config.steam.username.clear();
            config
                .save(&self.paths.config_file)
                .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
            if let Ok(mut guard) = self.config.lock() {
                *guard = config;
            }
        }
        // La session partagée vivait du jeton supprimé : la prochaine lecture
        // en ouvrira une neuve avec le compte reconnecté.
        lapinux_steam::shared::block_on(lapinux_steam::shared::forget());
        Ok(Envelope::ok(&self.steam_status()))
    }

    pub fn session_path(&self) -> std::path::PathBuf {
        session::path_in(&self.paths.data_dir)
    }

    pub(super) fn saved_session(&self, username: &str) -> Option<SavedSession> {
        session::load_from(&self.session_path())
            .filter(|saved| saved.account_name.eq_ignore_ascii_case(username))
    }

    pub(super) fn update_login(&self, update: impl FnOnce(&mut SteamLoginState)) {
        if let Ok(mut state) = self.login.lock() {
            update(&mut state);
        }
    }

    pub(super) fn finish_login(&self, state: SteamLoginState) {
        if let Ok(mut slot) = self.login.lock() {
            *slot = state;
        }
    }
}
