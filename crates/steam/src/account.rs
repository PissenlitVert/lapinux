//! Interface commune aux backends de compte de Lapinux.
//!
//! Deux chemins coexistent : le moteur de Valve (`steamclient.so`, qui sait
//! ouvrir une session locale mais pas s'authentifier) et notre client du
//! protocole CM (`crate::cm`, qui s'authentifie réellement). La commande
//! `lapinux-probe account` les interroge derrière le même contrat.

use crate::cm;

/// Compte obtenu par un backend, avec les détails utiles au diagnostic.
#[derive(Debug, Clone, Default)]
pub struct Account {
    pub backend: &'static str,
    pub steam_id: Option<u64>,
    /// Vrai seulement si le backend a réellement authentifié la session.
    pub logged_on: bool,
    pub anonymous: bool,
    pub endpoint: Option<String>,
    /// `None` si le backend ne sait pas lire les licences.
    pub licenses: Option<Vec<u32>>,
    /// Jeton de refresh à enregistrer pour la prochaine connexion.
    pub refresh_token: Option<String>,
    /// Donnée Steam Guard à réutiliser avec ce jeton.
    pub guard_data: Option<String>,
    pub details: Vec<(String, String)>,
}

/// Code Steam Guard qu'un backend peut demander pendant la connexion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prompt {
    EmailCode,
    TwoFactor,
    DeviceConfirmation,
    EmailConfirmation,
}

/// Un backend sait ouvrir une session et rendre un compte.
///
/// `prompt` n'est appelé que si le serveur exige un code Steam Guard ; le CLI
/// le lit au terminal et les tests fournissent une réponse fixe. Le booléen
/// indique qu'un code précédent a été refusé.
pub trait Backend {
    fn name(&self) -> &'static str;
    fn sign_in(
        &mut self,
        prompt: &mut dyn FnMut(Prompt, bool) -> Result<String, String>,
    ) -> Result<Account, String>;
}

/// Backend du protocole CM : connexion anonyme ou compte nominatif.
pub struct CmBackend {
    pub config: cm::Config,
    pub auth: cm::Auth,
}

impl CmBackend {
    pub fn new() -> Self {
        Self::anonymous()
    }

    pub fn anonymous() -> Self {
        Self {
            config: cm::Config::from_env(),
            auth: cm::Auth::Anonymous,
        }
    }

    /// Connexion nominative ; le mot de passe est déjà lu par l'appelant.
    pub fn account(name: String, password: String) -> Self {
        Self::account_with_guard(name, password, None)
    }

    /// Connexion nominative avec la donnée Steam Guard d'une session passée.
    pub fn account_with_guard(name: String, password: String, guard_data: Option<String>) -> Self {
        Self {
            config: cm::Config::from_env(),
            auth: cm::Auth::Account {
                name,
                password,
                guard_data,
            },
        }
    }

    /// Reconnexion sans mot de passe avec un jeton de refresh enregistré.
    pub fn token(name: String, refresh_token: String) -> Self {
        Self {
            config: cm::Config::from_env(),
            auth: cm::Auth::Token {
                name,
                refresh_token,
            },
        }
    }

    pub fn with_config(mut self, config: cm::Config) -> Self {
        self.config = config;
        self
    }
}

impl Default for CmBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for CmBackend {
    fn name(&self) -> &'static str {
        "cm"
    }

    fn sign_in(
        &mut self,
        prompt: &mut dyn FnMut(Prompt, bool) -> Result<String, String>,
    ) -> Result<Account, String> {
        let mut guard = |kind: cm::Guard, retry: bool| -> Result<String, cm::Error> {
            let kind = match kind {
                cm::Guard::EmailCode => Prompt::EmailCode,
                cm::Guard::TwoFactor => Prompt::TwoFactor,
                cm::Guard::DeviceConfirmation => Prompt::DeviceConfirmation,
                cm::Guard::EmailConfirmation => Prompt::EmailConfirmation,
            };
            prompt(kind, retry).map_err(cm::Error::Auth)
        };
        let session =
            cm::sign_in(&self.config, &self.auth, &mut guard).map_err(|error| error.to_string())?;
        let mut details = vec![
            ("eresult".to_string(), session.eresult.to_string()),
            (
                "canal".to_string(),
                format!(
                    "RSA-OAEP/SHA-1 + AES-256-CBC (nonce {} octets)",
                    session.challenge_len
                ),
            ),
        ];
        match &self.auth {
            cm::Auth::Account { name, .. } => {
                details.push(("compte".to_string(), name.clone()));
                details.push((
                    "authentification".to_string(),
                    "mot de passe + jeton".into(),
                ));
            }
            cm::Auth::Token { name, .. } => {
                details.push(("compte".to_string(), name.clone()));
                details.push(("authentification".to_string(), "jeton enregistré".into()));
            }
            cm::Auth::Anonymous => {}
        }
        if let Some(cell) = session.cell_id {
            details.push(("cell".to_string(), cell.to_string()));
        }
        if let Some(seconds) = session.heartbeat_seconds {
            details.push(("battement".to_string(), format!("{seconds}s")));
        }
        if let Some(country) = &session.country {
            details.push(("pays".to_string(), country.clone()));
        }
        if let Some(licenses) = &session.licenses {
            details.push(("licences".to_string(), licenses.len().to_string()));
        }
        Ok(Account {
            backend: "cm",
            steam_id: Some(session.steam_id),
            logged_on: true,
            anonymous: session.anonymous,
            endpoint: Some(session.endpoint),
            licenses: session.licenses.map(|licenses| {
                licenses
                    .into_iter()
                    .map(|license| license.package_id)
                    .collect()
            }),
            refresh_token: session.refresh_token,
            guard_data: session.guard_data,
            details,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cm_backend_has_a_stable_name() {
        assert_eq!(CmBackend::new().name(), "cm");
    }
}
