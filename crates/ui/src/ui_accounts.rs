//! Connexion Steam et gestion des comptes.

use super::*;

impl LapinuxApp {
    pub(super) fn open_steam_login(&mut self) {
        self.open_settings(SettingsTab::Connection);
        self.steam_password.clear();
        self.steam_token.clear();
        self.steam_guard.clear();
        if !self.steam_login_state.running {
            self.steam_login_state.error = None;
        }
        if let Some(status) = &self.steam_status {
            if !status.username.is_empty() {
                self.steam_user = status.username.clone();
            }
        }
    }

    /// Relit les dernières lignes du journal choisi tant que l'onglet Logs est
    /// ouvert. Le daemon valide le nom et ne lit que le dossier des journaux.
    pub(super) fn poll_logs(&mut self) {
        if !(self.settings_open && self.settings_tab == SettingsTab::Logs) {
            return;
        }
        if self.logs_poll.elapsed() < Duration::from_millis(1200) {
            return;
        }
        self.logs_poll = Instant::now();
        let request = Req::Logs {
            file: self.logs_file.clone(),
            lines: 400,
        };
        match self
            .client
            .request_with_timeout(&request, Duration::from_secs(5))
        {
            Ok(envelope) if envelope.ok => match envelope.into_data::<LogsView>() {
                Ok(view) => {
                    if !view.file.is_empty() {
                        self.logs_file = Some(view.file.clone());
                    }
                    self.logs = Some(view);
                    self.logs_error = None;
                }
                Err(error) => {
                    self.logs_error = Some(lapinux_core::i18n::t_args(
                        "accounts.unexpected_response",
                        &[("error", &error.to_string())],
                    ))
                }
            },
            Ok(envelope) => self.logs_error = envelope.error,
            Err(error) => self.logs_error = Some(error.to_string()),
        }
    }

    pub(super) fn submit_steam_login(&mut self) {
        if self.steam_login_state.running {
            self.notify(lapinux_core::i18n::t("accounts.already_connecting"));
            return;
        }
        let username = self.steam_user.trim().to_string();
        if username.is_empty() {
            self.notify(lapinux_core::i18n::t("accounts.enter_account"));
            return;
        }
        let guard = self.steam_guard.trim();
        let token = self.steam_token.trim();
        match self.client.request(&Req::SteamLogin {
            username: username.clone(),
            password: self.steam_password.clone(),
            guard: (!guard.is_empty()).then(|| guard.to_string()),
            token: (!token.is_empty()).then(|| token.to_string()),
        }) {
            Ok(envelope) if envelope.ok => {
                self.login_since = Some(Instant::now());
                self.steam_login_state = SteamLoginState {
                    running: true,
                    stage: "connexion".into(),
                    ..Default::default()
                };
            }
            Ok(envelope) => {
                self.notify(
                    envelope
                        .error
                        .unwrap_or_else(|| lapinux_core::i18n::t("accounts.refused").into()),
                );
            }
            Err(err) => self.notify(format!("{err}")),
        }
    }

    /// Supprime le jeton Lapinux enregistré : la prochaine connexion demandera
    /// un mot de passe (le reste du compte Steam n'est pas concerné).
    pub(super) fn forget_steam_token(&mut self) {
        match self.client.request(&Req::SteamForgetToken) {
            Ok(envelope) if envelope.ok => {
                let status = envelope.into_data::<SteamStatus>().ok();
                self.apply_steam_status(status);
                self.notify(lapinux_core::i18n::t("accounts.token_forgotten"));
            }
            Ok(envelope) => self.notify(
                envelope
                    .error
                    .unwrap_or_else(|| lapinux_core::i18n::t("accounts.token_kept").into()),
            ),
            Err(err) => self.notify(format!("{err}")),
        }
    }

    /// Déconnecte le compte Steam : le jeton, le nom du compte et la session
    /// sociale partent ensemble, comme le bouton « Déconnecter » des autres
    /// boutiques. La prochaine connexion repart d'un compte vierge.
    pub(super) fn disconnect_steam(&mut self) {
        match self.client.request(&Req::SteamDisconnect) {
            Ok(envelope) if envelope.ok => {
                let status = envelope.into_data::<SteamStatus>().ok();
                self.apply_steam_status(status);
                self.steam_user.clear();
                self.steam_password.clear();
                self.steam_guard.clear();
                self.steam_token.clear();
                self.steam_login_state = SteamLoginState::default();
                self.notify(lapinux_core::i18n::t("accounts.disconnected"));
            }
            Ok(envelope) => self.notify(
                envelope
                    .error
                    .unwrap_or_else(|| lapinux_core::i18n::t("accounts.disconnect_failed").into()),
            ),
            Err(err) => self.notify(format!("{err}")),
        }
    }

    /// Rétablit la jaquette automatique : l'image choisie à la main et les
    /// jaquettes téléchargées sont effacées, la fiche repart de la source
    /// préférée. C'est aussi le moyen de remplacer une jaquette communautaire
    /// qui ne correspondait pas au jeu.
    pub(super) fn restore_automatic_cover(&mut self, appid: u32) {
        let dir = self.paths.covers_dir();
        let mut cleared = false;
        for path in [
            self.paths.custom_cover_path(appid),
            self.paths.legacy_custom_cover_path(appid),
            dir.join(format!("{appid}.jpg")),
            dir.join(format!("sgdb-{appid}.jpg")),
        ] {
            if std::fs::remove_file(&path).is_ok() {
                cleared = true;
            }
        }
        self.covers.forget(appid);
        if cleared {
            self.notify(lapinux_core::i18n::t("library.cover_reset"));
        }
    }

    /// Ouvre une connexion par QR : le daemon publie l'URL du défi dès que
    /// Steam la rend, le QR suit dans l'écran de connexion.
    pub(super) fn start_steam_qr(&mut self) {
        if self.steam_login_state.running {
            self.notify(lapinux_core::i18n::t("accounts.already_connecting"));
            return;
        }
        match self.client.request(&Req::SteamLoginQr) {
            Ok(envelope) if envelope.ok => {
                self.login_since = Some(Instant::now());
                self.steam_qr = None;
                self.steam_login_state = SteamLoginState {
                    running: true,
                    stage: lapinux_core::i18n::t("accounts.qr_stage").into(),
                    ..Default::default()
                };
                self.login_poll = Instant::now() - Duration::from_secs(1);
            }
            Ok(envelope) => self.notify(
                envelope
                    .error
                    .unwrap_or_else(|| lapinux_core::i18n::t("accounts.qr_unavailable").into()),
            ),
            Err(err) => self.notify(format!("{err}")),
        }
    }

    /// Annule la tentative en cours : l'interface est libérée immédiatement,
    /// le fil réseau s'arrête dès qu'il rend la main.
    pub(super) fn cancel_steam_login(&mut self) {
        match self.client.request(&Req::SteamLoginCancel) {
            Ok(envelope) if envelope.ok => {
                let status = envelope.into_data::<SteamStatus>().ok();
                self.apply_steam_status(status);
                self.steam_guard.clear();
                self.steam_password.clear();
                self.notify(lapinux_core::i18n::t("accounts.cancelled"));
            }
            Ok(envelope) => self.notify(
                envelope
                    .error
                    .unwrap_or_else(|| lapinux_core::i18n::t("accounts.nothing_to_cancel").into()),
            ),
            Err(err) => self.notify(format!("{err}")),
        }
    }

    pub(super) fn submit_steam_guard(&mut self) {
        let code = self.steam_guard.trim().to_string();
        if code.is_empty() {
            self.notify(lapinux_core::i18n::t("ui.guard_required"));
            return;
        }
        match self.client.request(&Req::SteamLoginGuard { code }) {
            Ok(envelope) if envelope.ok => {
                self.steam_guard.clear();
                self.steam_login_state.guard = None;
                self.steam_login_state.stage = lapinux_core::i18n::t("accounts.verifying").into();
            }
            Ok(envelope) => self.notify(
                envelope
                    .error
                    .unwrap_or_else(|| lapinux_core::i18n::t("accounts.guard_refused").into()),
            ),
            Err(err) => self.notify(format!("{err}")),
        }
    }
}
