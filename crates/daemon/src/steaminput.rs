//! Steam Input et Atelier d'un jeu, lus dans les bibliothèques Steam.
//!
//! Rien n'est déplacé : les profils Steam Input sont des fichiers de Steam, et
//! l'Atelier ne gagne qu'un dossier `Mods/<jeu> (<appid>)/SteamWorkshop` de
//! vrais fichiers issus du contenu déjà installé.

use crate::{Result, Shared};
use lapinux_core::i18n::{t, t_args};
use lapinux_core::ipc::Envelope;
use lapinux_core::model::{DeviceInfo, SteamInputCommunityProfile, WorkshopAction};
use lapinux_steam::Library;
use std::path::PathBuf;

impl Shared {
    /// Bibliothèques Steam connues, y compris celle que Lapinux gère.
    pub(crate) fn steam_libraries(&self) -> Vec<Library> {
        let roots = lapinux_core::paths::steam_roots();
        let config = self.config_snapshot();
        let managed = config.library_root();
        lapinux_steam::discover_libraries_with_managed(&roots, Some(managed.as_path()))
    }

    /// Manettes réelles branchées, sans les pads virtuels de Lapinux.
    ///
    /// Le pad virtuel créé pour le remapping ne doit pas choisir les profils
    /// Steam Input : ce n'est pas une manette de l'utilisateur. Le pad de test
    /// du CLI est aussi écarté.
    fn gamepad_devices(&self) -> Vec<DeviceInfo> {
        lapinux_input::device::list_cached()
            .into_iter()
            .filter(|device| {
                !(device.name == format!("{} Virtual Pad", lapinux_core::APP_NAME)
                    || (device.vendor == 0x1234 && device.product == 0x5678))
            })
            .collect()
    }

    /// Profils Steam Input proposés pour les manettes branchées.
    pub fn steam_input(&self, appid: u32) -> Result<Envelope> {
        let roots = lapinux_core::paths::steam_roots();
        let report = lapinux_steam::steaminput::report_lapinux(
            &roots,
            &self.steam_libraries(),
            &self.gamepad_devices(),
            appid,
            &self.paths.data_dir,
        );
        Ok(Envelope::ok(&report))
    }

    /// Profil Steam Input sélectionné pour ce jeu, utilisé aussi par le
    /// lancement direct : le VDF est converti en mappings Lapinux, sans client
    /// Steam et sans injecter de souris.
    pub(crate) fn active_steam_input_profile(&self, appid: u32) -> Option<PathBuf> {
        let roots = lapinux_core::paths::steam_roots();
        let devices = self.gamepad_devices();
        lapinux_steam::steaminput::active_lapinux_profile_path(
            &roots,
            &self.steam_libraries(),
            &devices,
            appid,
            &self.paths.data_dir,
        )
    }

    /// Configurations que Lapinux a recopiées chez lui après application.
    pub fn steam_input_saved(&self) -> Result<Envelope> {
        let saved = lapinux_steam::steaminput::saved(&self.paths.manettes_dir());
        Ok(Envelope::ok(&saved))
    }

    /// Cherche les configurations publiées sur Steam, sans toucher au disque.
    pub fn steam_input_community(
        &self,
        appid: u32,
        query: &str,
        controller: Option<&str>,
        page: usize,
    ) -> Result<Envelope> {
        let report = lapinux_steam::steaminput::community(
            &self.paths.data_dir,
            appid,
            query,
            controller,
            page,
        )
        .map_err(lapinux_core::Error::Other)?;
        Ok(Envelope::ok(&report))
    }

    /// Télécharge une configuration communautaire et l'applique comme une
    /// autosave locale, sans abonnement Atelier.
    pub fn steam_input_community_apply(
        &self,
        appid: u32,
        profile: &SteamInputCommunityProfile,
    ) -> Result<Envelope> {
        let roots = lapinux_core::paths::steam_roots();
        let report = lapinux_steam::steaminput::apply_community(
            &roots,
            &self.steam_libraries(),
            &self.gamepad_devices(),
            appid,
            &self.paths.data_dir,
            profile,
        )
        .map_err(lapinux_core::Error::Other)?;
        Ok(Envelope::ok(&report))
    }

    /// Enregistre une copie sélectionnée dans Lapinux, sans modifier Steam.
    pub fn steam_input_apply(&self, appid: u32, profile: &std::path::Path) -> Result<Envelope> {
        let roots = lapinux_core::paths::steam_roots();
        let report = lapinux_steam::steaminput::apply_lapinux(
            &roots,
            &self.steam_libraries(),
            &self.gamepad_devices(),
            appid,
            &self.paths.data_dir,
            profile,
        )
        .map_err(lapinux_core::Error::Other)?;
        Ok(Envelope::ok(&report))
    }

    /// Atelier du jeu : abonnements, poids et dossier de rangement.
    ///
    /// `refresh` relit aussi les abonnements sur le site communautaire : un
    /// article suivi depuis la fenêtre web de Lapinux n'est pas toujours écrit
    /// dans `userdata` tant que le client Steam ne l'a pas vu. Une relecture en
    /// ligne impossible ne vide pas la fiche : la lecture locale reste, avec sa
    /// note.
    pub fn steam_workshop(&self, appid: u32, name: &str, refresh: bool) -> Result<Envelope> {
        let libraries = self.steam_libraries();
        let mut note = None;
        let online = if refresh {
            match lapinux_steam::session::load_from(&self.session_path()) {
                Some(saved) if !saved.refresh_token.trim().is_empty() => {
                    match lapinux_steam::web::workshop_subscriptions(
                        &saved.account_name,
                        &saved.refresh_token,
                        saved.steam_id,
                        appid,
                    ) {
                        Ok(ids) => {
                            tracing::info!(
                                "atelier {appid} : {} abonnement(s) relu(s) en ligne",
                                ids.len()
                            );
                            ids
                        }
                        Err(error) => {
                            tracing::warn!(
                                "atelier {appid} : relecture en ligne impossible ({error})"
                            );
                            note = Some(t_args(
                                "steaminput.note_local_refresh",
                                &[("error", &error.to_string())],
                            ));
                            Vec::new()
                        }
                    }
                }
                _ => {
                    note = Some(t("steaminput.note_no_session").to_string());
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        let mut report =
            lapinux_steam::workshop::report_with_subscriptions(&libraries, appid, name, &online);
        if let Some(note) = note {
            report.notes.push(note);
        }
        Ok(Envelope::ok(&report))
    }

    /// Range les abonnements dans `Mods/<jeu> (<appid>)/SteamWorkshop`.
    pub fn steam_workshop_arrange(&self, appid: u32, name: &str) -> Result<Envelope> {
        let report = lapinux_steam::workshop::arrange(&self.steam_libraries(), appid, name)
            .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
        Ok(Envelope::ok(&report))
    }

    /// Active, désactive ou supprime des contenus de l'Atelier. Les fichiers
    /// Steam ne sont jamais modifiés pour activer/désactiver ; la suppression
    /// ne touche qu'au dossier de contenu de l'appid et laisse la souscription
    /// Steam intacte.
    pub fn steam_workshop_manage(
        &self,
        appid: u32,
        name: &str,
        ids: &[u64],
        action: WorkshopAction,
    ) -> Result<Envelope> {
        let libraries = self.steam_libraries();
        let report = match action {
            WorkshopAction::Enable => {
                lapinux_steam::workshop::set_enabled(&libraries, appid, name, ids, true)
            }
            WorkshopAction::Disable => {
                lapinux_steam::workshop::set_enabled(&libraries, appid, name, ids, false)
            }
            WorkshopAction::Uninstall => {
                lapinux_steam::workshop::uninstall(&libraries, appid, name, ids)
            }
        }
        .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
        Ok(Envelope::ok(&report))
    }

    /// Télécharge des contenus sans ouvrir le client Steam, avec
    /// DepotDownloader, puis les range dans `Mods`.
    pub fn steam_workshop_download(&self, appid: u32, name: &str, ids: &[u64]) -> Result<Envelope> {
        let report = lapinux_steam::workshop::download_without_steam(
            &self.paths.data_dir,
            &self.steam_libraries(),
            appid,
            name,
            ids,
        )
        .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
        Ok(Envelope::ok(&report))
    }
}
