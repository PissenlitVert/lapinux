//! Création GPU des captures de test, sérialisée pour tout le processus.
//!
//! Le chargeur Vulkan scanne ses ICD une fois, sans verrou : deux
//! `wgpu::Instance::new` simultanés — les captures Actualités et Steam Input
//! tournent en parallèle sous le harnais de test — plantent dans
//! `loader_icd_scan`, au milieu de l'initialisation du pilote graphique.
//! Chaque capture garde donc ce verrou pendant tout son rendu.

use std::sync::{Mutex, MutexGuard, OnceLock};

pub(crate) type Guard = MutexGuard<'static, ()>;

/// Attend son tour puis réserve le GPU jusqu'au `drop` du garde.
pub(crate) fn serialized() -> Guard {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        // Une capture qui échoue empoisonne le verrou : les suivantes doivent
        // quand même tourner, pour rapporter leurs propres différences.
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
