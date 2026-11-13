//! Page Manettes et commandes associées.

use super::*;

impl LapinuxApp {
    /// Relit les configurations que Lapinux a gardées après application.
    fn poll_saved_steam_inputs(&mut self) {
        let received = self.saved_steam_inputs_rx.as_ref().map(|rx| rx.try_recv());
        match received {
            Some(Ok(saved)) => {
                if self.saved_steam_inputs != saved {
                    self.saved_steam_input_contents.clear();
                }
                self.saved_steam_inputs = saved;
                self.saved_steam_inputs_rx = None;
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.saved_steam_inputs_rx = None;
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }
    }

    /// Demande les copies conservées hors du fil d'affichage. La page
    /// Manettes peut ainsi être ouverte sans attendre un accès disque.
    fn request_saved_steam_inputs(&mut self, ctx: &egui::Context) {
        if self.saved_steam_inputs_rx.is_some() {
            return;
        }
        self.saved_steam_inputs_poll = Instant::now();
        let client = self.client.detached();
        let (tx, rx) = mpsc::channel();
        self.saved_steam_inputs_rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let saved = client
                .request(&Req::SteamInputSaved)
                .ok()
                .and_then(|envelope| envelope.into_data::<Vec<SteamInputSaved>>().ok())
                .unwrap_or_default();
            let _ = tx.send(saved);
            ctx.request_repaint();
        });
    }

    pub(super) fn controllers_page(&mut self, ui: &mut egui::Ui) {
        if self.pad_monitor.is_none() && self.session.is_none() && self.launching.is_none() {
            self.pad_monitor = lapinux_input::PadMonitor::start().ok();
        }
        self.poll_paired_pads();
        self.poll_pad_power(ui.ctx());
        self.poll_steam_lizard_install();
        self.poll_steam_lizard_persist();
        if self.steam_bridge.poll(ui.ctx()) {
            self.steam_puck_bridge = lapinux_input::device::steam_puck_bridge_running();
            self.refresh_async(ui.ctx(), false);
        }
        self.poll_saved_steam_inputs();
        if self.saved_steam_inputs_poll.elapsed() >= Duration::from_secs(5) {
            self.request_saved_steam_inputs(ui.ctx());
        }
        // Le Puck est un dongle USB : un branchement à chaud doit apparaître
        // sans redémarrer l'application, mais pas au rythme des images.
        if self.steam_puck_poll.elapsed() >= Duration::from_secs(2) {
            self.steam_puck_present = lapinux_input::device::steam_puck_present();
            self.steam_puck_bridge = lapinux_input::device::steam_puck_bridge_running();
            self.steam_lizard_mode = lapinux_input::device::steam_lizard_mode();
            self.steam_lizard_writable = lapinux_input::device::steam_lizard_writable();
            self.steam_lizard_persistent = lapinux_input::device::steam_lizard_config_installed();
            self.steam_puck_poll = Instant::now();
        }
        let evdev_paths: Vec<PathBuf> = self
            .devices
            .iter()
            .filter(|device| device.gamepad)
            .map(|device| device.path.clone())
            .collect();
        if self.pad_evdev_paths != evdev_paths {
            self.pad_evdev_paths = evdev_paths;
            self.paired_pads = None;
        }
        if self.paired_pads.is_none() {
            self.request_paired_pads(ui.ctx());
        }
        let pads = self
            .pad_monitor
            .as_ref()
            .map(|monitor| monitor.snapshot())
            .unwrap_or_default();
        // Un récepteur 2,4 GHz garde son nœud evdev même manette éteinte :
        // seule une entrée récente prouve que la manette est réveillée.
        let now = lapinux_core::util::now_unix().max(0) as u64;
        let evdev: Vec<DeviceInfo> = self
            .devices
            .iter()
            .filter(|device| device.gamepad)
            .cloned()
            .map(|mut device| {
                if dongle_asleep(&device, &pads, now) {
                    device.connected = false;
                }
                device
            })
            .collect();
        let mut gamepads = merge_controllers(evdev, self.paired_pads.clone().unwrap_or_default());
        // Le Puck n'expose pas de gamepad natif tant que hid-steam ne le
        // prend pas en charge : on lui garde une entrée appairée stable,
        // sélectionnée par défaut, pour porter le diagnostic et le pont.
        if steam_puck_needs_entry(self.steam_puck_present, self.steam_puck_bridge, &gamepads) {
            gamepads.insert(0, steam_puck_placeholder());
        }

        // La sélection doit exister dans la liste : au premier affichage, on
        // prend la première manette connectée, sinon la première appairée.
        // Une liste momentanément vide (le temps qu'un accès hidraw referme
        // le nœud evdev) ne doit pas effacer la sélection : sinon le test en
        // direct et le relevé HID disparaîtraient à chaque impulsion.
        if !gamepads.is_empty()
            && self.pad_selected.as_ref().map_or(true, |key| {
                !gamepads
                    .iter()
                    .any(|pad| controller_key(pad) == key.as_str())
            })
        {
            self.pad_selected = gamepads
                .iter()
                .find(|pad| pad.connected)
                .or_else(|| gamepads.first())
                .map(controller_key);
        }
        let mut selected = self
            .pad_selected
            .as_ref()
            .and_then(|key| {
                gamepads
                    .iter()
                    .find(|pad| controller_key(pad) == key.as_str())
            })
            .cloned();
        let mut selected_pad = selected
            .as_ref()
            .and_then(|device| pads.iter().find(|pad| pad.path == device.path).cloned());

        // Ouvrir hidraw retire temporairement event27. On garde alors la
        // dernière fiche complète pendant quelques secondes : la page, son
        // défilement et les cartes de test ne bougent pas pendant le relevé.
        let holding = self.pad_tests.steam_raw_rx.is_some()
            || self.pad_tests.steam_grip_rx.is_some()
            || self
                .pad_tests
                .steam_hold_until
                .is_some_and(|until| until > Instant::now());
        if let (Some(device), Some(pad)) = (selected.as_ref(), selected_pad.as_ref()) {
            self.pad_cache = Some(PadCache {
                key: controller_key(device),
                device: device.clone(),
                pad: pad.clone(),
            });
        }
        if holding {
            if let Some(cache) = self
                .pad_cache
                .as_ref()
                .filter(|cache| self.pad_selected.as_deref() == Some(cache.key.as_str()))
            {
                selected = Some(cache.device.clone());
                selected_pad = Some(cache.pad.clone());
            }
        }
        let mut display_gamepads = gamepads.clone();
        if let Some(device) = selected.as_ref() {
            let key = controller_key(device);
            if let Some(slot) = display_gamepads
                .iter_mut()
                .find(|pad| controller_key(pad) == key)
            {
                *slot = device.clone();
            } else {
                display_gamepads.push(device.clone());
            }
        }

        // Un changement de manette remet les tests à zéro ; une disparition
        // temporaire de la même manette les conserve au contraire.
        if let Some(device) = selected.as_ref() {
            let key = controller_key(device);
            if self.pad_tests_for.as_deref() != Some(key.as_str()) {
                self.pad_tests = PadTestState::default();
                self.pad_tests_for = Some(key);
            }
        }

        // Les tests audio ne concernent que la manette sélectionnée et en
        // ligne. Le détecteur est recréé quand la sélection change ; sinon il
        // rafraîchit ses routes sans bloquer l'interface à chaque image.
        if let Some(device) = selected.as_ref().filter(|device| device.connected) {
            let key = controller_key(device);
            let changed = self
                .pad_audio
                .as_ref()
                .map_or(true, |audio| audio.key != key);
            if changed {
                let identity = pad_identity(&device);
                let with_mic = controller_features(device, &identity, selected_pad.as_ref())
                    .contains(&"Microphone");
                self.pad_audio = Some(PadAudio::new(key, device, with_mic));
            } else if let Some(audio) = self.pad_audio.as_mut() {
                audio.refresh(device);
            }
        } else {
            self.pad_audio = None;
        }

        if let Some(tab) = controllers_tab_bar(ui, self.controllers_tab) {
            self.controllers_tab = tab;
        }
        ui.add_space(12.0);
        if self.controllers_tab == ControllersTab::General {
            self.controllers_general(ui);
            return;
        }

        // Deux colonnes comme les téléchargements : la liste à gauche, le
        // détail et le test de la manette choisie à droite.
        let available = ui.available_width();
        // La liste porte désormais le bouton d'alimentation de chaque manette :
        // elle gagne un peu de largeur pour garder les noms lisibles.
        let left_width = (available * 0.32).clamp(296.0, 356.0);
        let right_width = (available - left_width - 16.0).max(360.0);
        let height = ui.available_height();

        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(left_width, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(left_width);
                    self.controllers_list(ui, &display_gamepads, &pads);
                },
            );
            ui.add_space(8.0);
            ui.allocate_ui_with_layout(
                egui::vec2(right_width, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(right_width);
                    egui::ScrollArea::vertical()
                        .id_salt("controllers-detail")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            // Le défilement occupe le bord droit : on réserve
                            // sa largeur pour que les cartes ne débordent pas.
                            ui.set_width((right_width - 14.0).max(300.0));
                            match &selected {
                                Some(device) if is_steam_puck_placeholder(device) => {
                                    self.controllers_steam_puck_detail(ui)
                                }
                                Some(device) if device.steam_bridge => {
                                    Self::steam_bridge_card(ui, &mut self.steam_bridge);
                                    ui.add_space(10.0);
                                    self.controller_detail(ui, device, selected_pad.as_ref())
                                }
                                Some(device) => {
                                    self.controller_detail(ui, device, selected_pad.as_ref())
                                }
                                None => self.controllers_empty(ui),
                            }
                        });
                },
            );
        });
        // Le test doit rester fluide même sans mouvement de souris.
        ui.ctx().request_repaint_after(Duration::from_millis(33));
    }

    /// Onglet Général : le profil d'affichage des boutons de façade. Le choix
    /// s'applique tout de suite et part au daemon pour survivre au redémarrage.
    fn controllers_general(&mut self, ui: &mut egui::Ui) {
        if let Some(rx) = &self.saved_steam_input_contents_rx {
            match rx.try_recv() {
                Ok((path, contents)) => {
                    self.saved_steam_input_contents.insert(path, contents);
                    self.saved_steam_input_contents_rx = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => self.saved_steam_input_contents_rx = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let mut load_profile = None;
        let selected = self.config.input.face_profile;
        let mut chosen = None;
        egui::ScrollArea::vertical()
            .id_salt("controllers-general")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Colonne de largeur bornée, centrée : les profils ne restent
                // pas collés au bord gauche quand la fenêtre est large.
                let width = ui.available_width().min(920.0);
                let side = ((ui.available_width() - width) * 0.5).max(0.0);
                egui::Frame::none()
                    .inner_margin(egui::Margin::symmetric(side, 0.0))
                    .show(ui, |ui| {
                        ui.set_width(width);
                        settings_card(
                            ui,
                            lapinux_core::i18n::t("controllers.face_buttons"),
                            lapinux_core::i18n::t("controllers.face_desc"),
                            |ui| {
                                let width = ui.available_width();
                                let gap = 12.0;
                                let card_width = ((width - gap) / 2.0).max(220.0);
                                ui.spacing_mut().item_spacing.x = gap;
                                ui.horizontal_top(|ui| {
                                    for profile in
                                        [FaceButtonProfile::Xbox, FaceButtonProfile::Nintendo]
                                    {
                                        if face_profile_card(
                                            ui,
                                            profile,
                                            profile == selected,
                                            card_width,
                                        )
                                        .clicked()
                                        {
                                            chosen = Some(profile);
                                        }
                                    }
                                });
                                ui.add_space(8.0);
                                muted(ui, lapinux_core::i18n::t("controllers.face_note"));
                            },
                        );
                        ui.add_space(12.0);
                        settings_card(
                            ui,
                            lapinux_core::i18n::t("controllers.saved_profiles_title"),
                            lapinux_core::i18n::t("controllers.saved_profiles_hint"),
                            |ui| {
                                if self.saved_steam_inputs.is_empty() {
                                    muted(
                                        ui,
                                        lapinux_core::i18n::t("controllers.saved_profiles_empty"),
                                    );
                                }
                                for saved in &self.saved_steam_inputs {
                                    egui::CollapsingHeader::new(format!(
                                        "{} · {} · {}",
                                        saved.title,
                                        self.game_name(saved.appid),
                                        lapinux_core::model::steam_input_label(
                                            &saved.controller_type
                                        )
                                    ))
                                    .id_salt(("general-saved-profile", &saved.path))
                                    .show(ui, |ui| {
                                        if !saved.description.is_empty() {
                                            muted(ui, &saved.description);
                                        }
                                        // Le détail structuré est la vue
                                        // principale ; le VDF brut reste
                                        // disponible pour le diagnostic.
                                        crate::steaminput::saved_action_details(ui, saved);
                                        egui::CollapsingHeader::new(
                                            lapinux_core::i18n::t("steaminput.raw_vdf"),
                                        )
                                        .id_salt(("general-profile-raw", &saved.path))
                                        .show(ui, |ui| {
                                            match self.saved_steam_input_contents.get(&saved.path) {
                                                Some(Ok(lines)) => {
                                                    egui::ScrollArea::both()
                                                        .id_salt((
                                                            "general-profile-content",
                                                            &saved.path,
                                                        ))
                                                        .max_height(320.0)
                                                        .show_rows(
                                                            ui,
                                                            15.0,
                                                            lines.len(),
                                                            |ui, range| {
                                                                for line in &lines[range] {
                                                                    ui.add(
                                                                        egui::Label::new(
                                                                            egui::RichText::new(line)
                                                                                .monospace()
                                                                                .size(10.0),
                                                                        )
                                                                        .wrap_mode(
                                                                            egui::TextWrapMode::Extend,
                                                                        ),
                                                                    );
                                                                }
                                                            },
                                                        );
                                                }
                                                Some(Err(error)) => {
                                                    ui.label(
                                                        egui::RichText::new(error).color(WARN),
                                                    );
                                                }
                                                None => {
                                                    muted(
                                                        ui,
                                                        lapinux_core::i18n::t(
                                                            "controllers.saved_profiles_loading",
                                                        ),
                                                    );
                                                    if self.saved_steam_input_contents_rx.is_none() {
                                                        load_profile = Some(saved.path.clone());
                                                    }
                                                }
                                            }
                                        });
                                    });
                                    ui.add_space(4.0);
                                }
                            },
                        );
                    });
            });
        if let Some(path) = load_profile {
            let (tx, rx) = mpsc::channel();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                use std::io::Read;
                const LIMIT: u64 = 2 * 1024 * 1024;
                let result = (|| -> std::io::Result<String> {
                    let mut bytes = Vec::new();
                    std::fs::File::open(&path)?
                        .take(LIMIT + 1)
                        .read_to_end(&mut bytes)?;
                    if bytes.len() as u64 > LIMIT {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "VDF > 2 MiB",
                        ));
                    }
                    String::from_utf8(bytes).map_err(|error| {
                        std::io::Error::new(std::io::ErrorKind::InvalidData, error)
                    })
                })()
                .map(|text| text.lines().map(str::to_owned).collect())
                .map_err(|error| error.to_string());
                let _ = tx.send((path, result));
                ctx.request_repaint();
            });
            self.saved_steam_input_contents_rx = Some(rx);
        }
        if let Some(profile) = chosen {
            if profile != self.config.input.face_profile {
                let mut config = self.config.clone();
                config.input.face_profile = profile;
                if self.apply_config(config) {
                    self.notify(lapinux_core::i18n::t_args(
                        "controllers.profile_applied",
                        &[("profile", face_profile_label(profile))],
                    ));
                }
            }
        }
    }

    /// Récupère le résultat de la coupure Bluetooth lancée en arrière-plan.
    /// Éteint une manette Bluetooth (deux temps) ou reconnecte une manette
    /// appairée hors ligne. Tout se passe hors du fil d'affichage.
    fn request_pad_power(&mut self, device: &DeviceInfo, ctx: &egui::Context) {
        let key = controller_key(device);
        if self
            .pad_power
            .as_ref()
            .map_or(false, |power| power.key == key && power.rx.is_some())
        {
            return;
        }
        if device.connected {
            if device.connection != DeviceLink::Bluetooth {
                return;
            }
            let armed = self.pad_power.as_ref().map_or(false, |power| {
                power.key == key
                    && power.kind == PowerKind::Off
                    && power
                        .armed_until
                        .map_or(false, |until| until > Instant::now())
            });
            if armed {
                let address = device.address.clone().unwrap_or_default();
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(lapinux_input::bluetooth::power_off(&address));
                });
                self.pad_power = Some(PadPower {
                    key,
                    kind: PowerKind::Off,
                    armed_until: None,
                    rx: Some(rx),
                });
            } else {
                self.pad_power = Some(PadPower {
                    key,
                    kind: PowerKind::Off,
                    armed_until: Some(Instant::now() + Duration::from_secs(4)),
                    rx: None,
                });
            }
        } else {
            let address = device.address.clone().unwrap_or_default();
            if address.is_empty() {
                self.notify(lapinux_core::i18n::t("controllers.power.no_address"));
                return;
            }
            self.notify(lapinux_core::i18n::t("controllers.power.searching"));
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(lapinux_input::bluetooth::connect(&address));
            });
            self.pad_power = Some(PadPower {
                key,
                kind: PowerKind::On,
                armed_until: None,
                rx: Some(rx),
            });
        }
        ctx.request_repaint();
    }

    /// Coupe le Bluetooth pour laisser le câble USB prendre le relais.
    ///
    /// Le noyau refuse de créer la manette filaire tant que le Bluetooth est
    /// actif (même adresse). La coupure est donc la première moitié de la
    /// bascule ; il faut ensuite rebrancher le câble pour que le pilote
    /// reprobe l'USB.
    fn request_wired_handoff(&mut self, device: &DeviceInfo, ctx: &egui::Context) {
        let key = controller_key(device);
        if self
            .pad_power
            .as_ref()
            .map_or(false, |power| power.key == key && power.rx.is_some())
        {
            return;
        }
        let address = device.address.clone().unwrap_or_default();
        if address.is_empty() {
            self.notify("adresse Bluetooth inconnue");
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(lapinux_input::bluetooth::power_off(&address));
        });
        self.pad_power = Some(PadPower {
            key,
            kind: PowerKind::Wired,
            armed_until: None,
            rx: Some(rx),
        });
        ctx.request_repaint();
    }

    fn poll_pad_power(&mut self, ctx: &egui::Context) {
        let Some(power) = self.pad_power.as_ref() else {
            return;
        };
        let Some(rx) = power.rx.as_ref() else {
            return;
        };
        let key = power.key.clone();
        let kind = power.kind;
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err(lapinux_core::i18n::t("controllers.power.bluez_silent").to_string())
            }
        };
        self.pad_power = None;
        match (kind, &result) {
            (PowerKind::Off, Ok(())) => self.notify(lapinux_core::i18n::t("controllers.power.off")),
            (PowerKind::Off, Err(error)) => self.notify(lapinux_core::i18n::t_args(
                "controllers.power.off_failed",
                &[("error", &error.to_string())],
            )),
            (PowerKind::Wired, Ok(())) => {
                self.notify(lapinux_core::i18n::t("controllers.power.wired"))
            }
            (PowerKind::Wired, Err(error)) => self.notify(lapinux_core::i18n::t_args(
                "controllers.power.wired_failed",
                &[("error", &error.to_string())],
            )),
            (PowerKind::On, Ok(())) => {
                self.notify(lapinux_core::i18n::t("controllers.power.connected"))
            }
            (PowerKind::On, Err(error)) => {
                // Garde l'échec visible une trentaine de secondes : le toast
                // seul disparaît trop vite pour expliquer la limite du PS.
                self.pad_power = Some(PadPower {
                    key,
                    kind: PowerKind::On,
                    armed_until: Some(Instant::now() + Duration::from_secs(30)),
                    rx: None,
                });
                self.notify(lapinux_core::i18n::t_args(
                    "controllers.power.connect_failed",
                    &[("error", &error.to_string())],
                ))
            }
        }
        // BlueZ vient de changer l'état : la liste « Appairées » doit la voir
        // tout de suite, sans attendre le balayage périodique.
        self.paired_pads = None;
        self.request_paired_pads(ctx);
        if result.is_ok() {
            self.refresh_async(ctx, false);
        }
    }

    /// Récupère le résultat de la découverte BlueZ lancée en arrière-plan.
    fn poll_paired_pads(&mut self) {
        let received = self.paired_rx.as_ref().map(|rx| rx.try_recv());
        match received {
            Some(Ok(pads)) => {
                self.paired_pads = Some(pads);
                self.paired_rx = None;
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.paired_rx = None;
                self.paired_pads = Some(Vec::new());
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }
    }

    /// Interroge BlueZ sans bloquer le rendu egui.
    fn request_paired_pads(&mut self, ctx: &egui::Context) {
        if self.paired_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.paired_rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let pads = lapinux_input::paired_gamepads();
            let _ = tx.send(pads);
            ctx.request_repaint();
        });
    }

    /// Fiche du Puck quand aucun gamepad Steam natif n'est exposé : le
    /// diagnostic et les commandes du pont sont réunis dans l'entrée appairée.
    fn controllers_steam_puck_detail(&mut self, ui: &mut egui::Ui) {
        steam_puck_card(ui);
        ui.add_space(10.0);
        if self
            .steam_bridge
            .visible(self.steam_puck_present, self.steam_puck_bridge)
        {
            Self::steam_bridge_card(ui, &mut self.steam_bridge);
        }
    }

    /// Carte du pont userspace : état, arrêt ou relance, et fichier du service.
    /// L'arrêt est explicite et ne change pas l'activation au démarrage.
    pub(super) fn steam_bridge_card(ui: &mut egui::Ui, bridge: &mut steam_bridge::SteamBridge) {
        let service = bridge.service.clone();
        let installed = service.as_ref().is_some_and(|service| service.installed());
        let running = service
            .as_ref()
            .filter(|service| service.installed() && service.error.is_none() && service.state_known)
            .map(|service| service.active);
        let can_control = running.is_some();
        let failed = service
            .as_ref()
            .is_some_and(|service| service.error.is_none() && service.failed);
        let busy = bridge.busy;
        let details = bridge.details;
        let note = bridge.note.clone();
        let mut restart = false;
        let mut stop = false;
        let mut toggle_details = false;
        let mut open_unit = false;

        let accent = if running == Some(true) { OK } else { WARN };
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, with_alpha(accent, 0.45)))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(egui::Margin::symmetric(12.0, 11.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (icon, _) =
                        ui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::hover());
                    ui.painter().rect_filled(
                        icon,
                        egui::Rounding::same(8.0),
                        with_alpha(accent, 0.10),
                    );
                    ui.painter().text(
                        icon.center(),
                        egui::Align2::CENTER_CENTER,
                        nav_glyph(Page::Controllers),
                        icon_font(18.0),
                        accent,
                    );
                    ui.add_space(2.0);
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t(
                                "controllers.steam.bridge_title",
                            ))
                            .size(12.5)
                            .strong()
                            .color(TEXT()),
                        );
                        ui.horizontal(|ui| {
                            let (dot, _) =
                                ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
                            ui.painter().circle_filled(dot.center(), 3.0, accent);
                            ui.label(
                                egui::RichText::new(match running {
                                    Some(false) if failed => {
                                        lapinux_core::i18n::t("controllers.steam.bridge_failed")
                                    }
                                    Some(true) => {
                                        lapinux_core::i18n::t("controllers.steam.bridge_running")
                                    }
                                    Some(false) => {
                                        lapinux_core::i18n::t("controllers.steam.bridge_stopped")
                                    }
                                    None => {
                                        lapinux_core::i18n::t("controllers.steam.bridge_unknown")
                                    }
                                })
                                .size(10.5)
                                .color(accent),
                            );
                        });
                    });
                });
                ui.add_space(6.0);
                muted(
                    ui,
                    match running {
                        Some(true) => {
                            lapinux_core::i18n::t("controllers.steam.bridge_body_running")
                        }
                        Some(false) if failed => {
                            lapinux_core::i18n::t("controllers.steam.bridge_body_failed")
                        }
                        Some(false) => {
                            lapinux_core::i18n::t("controllers.steam.bridge_body_stopped")
                        }
                        None if service.as_ref().is_some_and(|service| {
                            service.error.is_none() && !service.installed()
                        }) =>
                        {
                            lapinux_core::i18n::t("controllers.steam.bridge_body_missing")
                        }
                        None => lapinux_core::i18n::t("controllers.steam.bridge_state_unconfirmed"),
                    },
                );
                ui.add_space(7.0);
                ui.horizontal_wrapped(|ui| {
                    if running == Some(true) {
                        let response = action_button_enabled(
                            ui,
                            lapinux_core::i18n::t("controllers.steam.bridge_stop"),
                            ActionTone::Light,
                            can_control && !busy,
                        );
                        if response
                            .on_hover_text(lapinux_core::i18n::t(
                                "controllers.steam.bridge_stop_hint",
                            ))
                            .clicked()
                        {
                            stop = true;
                        }
                    }
                    let restart_label = if busy {
                        lapinux_core::i18n::t("controllers.steam.bridge_working")
                    } else if running == Some(true) {
                        lapinux_core::i18n::t("controllers.steam.bridge_restart")
                    } else {
                        lapinux_core::i18n::t("controllers.steam.bridge_start")
                    };
                    if action_button_enabled(
                        ui,
                        restart_label,
                        ActionTone::Light,
                        can_control && !busy,
                    )
                    .clicked()
                    {
                        restart = true;
                    }
                    if action_button(
                        ui,
                        if details {
                            lapinux_core::i18n::t("controllers.steam.bridge_service_hide")
                        } else {
                            lapinux_core::i18n::t("controllers.steam.bridge_service")
                        },
                        ActionTone::Quiet,
                    )
                    .clicked()
                    {
                        toggle_details = true;
                    }
                });
                if running == Some(true) {
                    ui.add_space(4.0);
                    muted(
                        ui,
                        lapinux_core::i18n::t("controllers.steam.bridge_stop_hint"),
                    );
                }
                if service
                    .as_ref()
                    .is_some_and(|service| service.error.is_none() && !service.installed())
                {
                    ui.add_space(4.0);
                    muted(
                        ui,
                        lapinux_core::i18n::t("controllers.steam.bridge_service_missing"),
                    );
                }
                if details {
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(4.0);
                    if let Some(service) = &service {
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t(
                                "controllers.steam.bridge_service_title",
                            ))
                            .size(11.0)
                            .strong()
                            .color(TEXT()),
                        );
                        if service.error.is_none() && installed && service.state_known {
                            ui.horizontal_wrapped(|ui| {
                                let (dot, _) = ui.allocate_exact_size(
                                    egui::vec2(9.0, 9.0),
                                    egui::Sense::hover(),
                                );
                                let color = if service.active { OK } else { WARN };
                                ui.painter().circle_filled(dot.center(), 3.0, color);
                                ui.label(
                                    egui::RichText::new(if service.failed {
                                        lapinux_core::i18n::t("controllers.steam.bridge_failed")
                                    } else if service.active {
                                        lapinux_core::i18n::t(
                                            "controllers.steam.bridge_service_active",
                                        )
                                    } else {
                                        lapinux_core::i18n::t(
                                            "controllers.steam.bridge_service_inactive",
                                        )
                                    })
                                    .size(10.5)
                                    .color(color),
                                );
                                ui.label(
                                    egui::RichText::new(if service.enabled {
                                        lapinux_core::i18n::t(
                                            "controllers.steam.bridge_service_enabled",
                                        )
                                    } else {
                                        lapinux_core::i18n::t(
                                            "controllers.steam.bridge_service_disabled",
                                        )
                                    })
                                    .size(10.5)
                                    .color(TEXT_DIM()),
                                );
                            });
                        }
                        ui.add_space(3.0);
                        if let Some(path) = &service.unit_path {
                            ui.label(
                                egui::RichText::new(path.display().to_string())
                                    .size(9.5)
                                    .monospace()
                                    .color(TEXT_DIM()),
                            );
                            ui.add_space(3.0);
                            if action_button(
                                ui,
                                lapinux_core::i18n::t("controllers.steam.bridge_open_unit"),
                                ActionTone::Quiet,
                            )
                            .clicked()
                            {
                                open_unit = true;
                            }
                        } else if service.error.is_none() && !installed {
                            muted(
                                ui,
                                lapinux_core::i18n::t("controllers.steam.bridge_service_missing"),
                            );
                        }
                        if let Some(error) = &service.error {
                            ui.add_space(3.0);
                            ui.label(egui::RichText::new(error).size(10.0).color(WARN));
                        }
                    } else {
                        muted(
                            ui,
                            lapinux_core::i18n::t("controllers.steam.bridge_service_reading"),
                        );
                    }
                }
                if let Some((text, ok)) = &note {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(text).size(10.5).color(if *ok {
                        OK
                    } else {
                        WARN
                    }));
                }
            });

        if toggle_details {
            bridge.details = !bridge.details;
        }
        if open_unit {
            if let Err(error) = bridge.open_unit() {
                bridge.note = Some((
                    lapinux_core::i18n::t_args(
                        "controllers.steam.bridge_open_failed",
                        &[("error", &error)],
                    ),
                    false,
                ));
            }
        }
        if stop {
            bridge.stop(ui.ctx());
        } else if restart {
            bridge.restart(ui.ctx());
        }
    }

    /// Liste verticale de gauche : les manettes connectées (USB, Bluetooth ou
    /// dongle), puis les manettes appairées mais éteintes.
    fn controllers_list(
        &mut self,
        ui: &mut egui::Ui,
        gamepads: &[DeviceInfo],
        pads: &[lapinux_input::PadState],
    ) {
        let width = ui.available_width();
        egui::ScrollArea::vertical()
            .id_salt("controllers-list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width((width - 8.0).max(220.0));
                if gamepads.is_empty() {
                    egui::Frame::none()
                        .fill(CARD())
                        .stroke(egui::Stroke::new(1.0_f32, with_alpha(WARN, 0.45)))
                        .rounding(egui::Rounding::same(12.0))
                        .inner_margin(16.0)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                egui::RichText::new(lapinux_core::i18n::t("controllers.none"))
                                    .strong()
                                    .color(WARN),
                            );
                            ui.add_space(4.0);
                            muted(ui, lapinux_core::i18n::t("controllers.none_hint"));
                        });
                    return;
                }

                let (online, paired) = split_controller_groups(gamepads);
                if !online.is_empty() {
                    let collapsed = &mut self.pads_collapsed[0];
                    controller_group_header(
                        ui,
                        lapinux_core::i18n::t("controllers.ui.online"),
                        online.len(),
                        collapsed,
                    );
                    if !*collapsed {
                        for device in &online {
                            let pad = pads.iter().find(|pad| pad.path == device.path);
                            let action = self.controller_row(ui, device, pad);
                            if action.select {
                                self.pad_selected = Some(controller_key(device));
                            }
                            if action.power {
                                let ctx = ui.ctx().clone();
                                self.request_pad_power(device, &ctx);
                            }
                            ui.add_space(6.0);
                        }
                    }
                }
                if !paired.is_empty() {
                    if !online.is_empty() {
                        ui.add_space(8.0);
                    }
                    let collapsed = &mut self.pads_collapsed[1];
                    controller_group_header(
                        ui,
                        lapinux_core::i18n::t("controllers.paired"),
                        paired.len(),
                        collapsed,
                    );
                    if !*collapsed {
                        for device in &paired {
                            let pad = pads.iter().find(|pad| pad.path == device.path);
                            let action = self.controller_row(ui, device, pad);
                            if action.select {
                                self.pad_selected = Some(controller_key(device));
                            }
                            if action.power {
                                let ctx = ui.ctx().clone();
                                self.request_pad_power(device, &ctx);
                            }
                            ui.add_space(6.0);
                        }
                    }
                }
            });
    }

    /// Une ligne de la liste : vignette, nom, connexion et batterie. Un clic
    /// sélectionne la manette dont le détail s'affiche à droite.
    pub(super) fn controller_row(
        &self,
        ui: &mut egui::Ui,
        device: &DeviceInfo,
        pad: Option<&lapinux_input::PadState>,
    ) -> ControllerRowAction {
        let identity = pad_identity(&device);
        let key = controller_key(device);
        let live = pad.and_then(|pad| pad.battery.as_ref());
        let battery = live.and_then(|battery| battery.percent).or(device.battery);
        let state = live
            .map(|battery| battery.state)
            .filter(|state| *state != ChargeState::Unknown)
            .or_else(|| {
                device
                    .battery_status
                    .as_deref()
                    .map(ChargeState::from_sysfs)
            });
        let charging = battery_charging(state);
        if charging {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
        let time = ui.input(|input| input.time);
        let selected = self
            .pad_selected
            .as_ref()
            .map_or(false, |selected| selected == &key);
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 58.0), egui::Sense::click());
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        let hovered = response.hovered();
        let painter = ui.painter();
        let stroke_color = if selected || response.has_focus() {
            ACCENT()
        } else {
            BORDER()
        };
        painter.rect(
            rect,
            egui::Rounding::same(10.0),
            if selected {
                CARD_ACTIVE()
            } else if hovered {
                CARD_HOVER()
            } else {
                CARD()
            },
            egui::Stroke::new(1.0_f32, stroke_color),
        );
        if selected {
            painter.rect_filled(
                egui::Rect::from_min_size(
                    rect.left_top() + egui::vec2(0.0, 13.0),
                    egui::vec2(3.0, rect.height() - 26.0),
                ),
                egui::Rounding::same(1.5),
                ACCENT(),
            );
        }

        // Vignette de la manette, avec une pastille d'état en bas à droite.
        let tile = egui::Rect::from_min_size(
            rect.left_top() + egui::vec2(10.0, 9.0),
            egui::vec2(40.0, 40.0),
        );
        painter.rect_filled(tile, egui::Rounding::same(9.0), BACKGROUND());
        painter.text(
            tile.center(),
            egui::Align2::CENTER_CENTER,
            nav_glyph(Page::Controllers),
            icon_font(24.0),
            if device.connected { TEXT() } else { TEXT_DIM() },
        );
        let dot = tile.right_bottom() + egui::vec2(-5.0, -5.0);
        painter.circle_filled(dot, 5.0, BACKGROUND());
        painter.circle_filled(dot, 3.2, if device.connected { OK } else { WARN });

        // Une manette en ligne se commande depuis la liste (éteindre), une
        // manette appairée hors ligne s'y rallume. Une manette USB n'a pas de
        // bouton : le câble l'alimente.
        let (power_pending, power_armed, power_failed) = self
            .pad_power
            .as_ref()
            .filter(|power| power.key == key)
            .map_or((false, false, false), |power| {
                (
                    power.rx.is_some(),
                    power.kind == PowerKind::Off
                        && power
                            .armed_until
                            .map_or(false, |until| until > Instant::now()),
                    power.kind == PowerKind::On
                        && power.rx.is_none()
                        && power
                            .armed_until
                            .map_or(false, |until| until > Instant::now()),
                )
            });
        let power = if device.connected {
            if device.connection == DeviceLink::Bluetooth {
                PadPowerControl::Off {
                    armed: power_armed,
                    pending: power_pending,
                }
            } else {
                PadPowerControl::Hidden
            }
        } else if device.address.is_some() {
            PadPowerControl::On {
                pending: power_pending,
                failed: power_failed,
            }
        } else {
            PadPowerControl::Hidden
        };
        let power_visible = !matches!(power, PadPowerControl::Hidden);
        let battery_width = if battery.is_some() { 30.0 } else { 0.0 };
        let connection_width = 30.0;
        let right_space = battery_width
            + connection_width
            + if battery.is_some() { 8.0 } else { 0.0 }
            + if power_visible { 36.0 } else { 0.0 };
        let brand_size = 20.0;
        let brand_advance = identity
            .brand
            .map_or(0.0, |brand| brand_mark_advance(painter, brand, brand_size));
        let logo_width = if identity.brand.is_some() {
            brand_advance + 5.0
        } else {
            0.0
        };
        let text_x = tile.right() + 10.0;
        let text_width = (rect.right() - 12.0 - right_space - logo_width - text_x).max(40.0);
        let name_font = display_font(12.5);
        let shown = pad_shown_name(&self.config.input.pad_names, device, &identity);
        let name = elide_to_width(painter, &shown, name_font.clone(), text_width);
        let name_width = painter
            .layout_no_wrap(name.clone(), name_font.clone(), TEXT())
            .size()
            .x;
        painter.text(
            egui::pos2(text_x, rect.top() + 12.0),
            egui::Align2::LEFT_TOP,
            name,
            name_font,
            if device.connected { TEXT() } else { TEXT_DIM() },
        );
        // Logo de marque juste après le nom, sans remplacer la manette.
        if let Some(brand) = identity.brand {
            paint_brand_mark(
                painter,
                brand,
                text_x + name_width + 5.0,
                rect.top() + 19.5,
                brand_size,
                if device.connected {
                    TEXT()
                } else {
                    with_alpha(TEXT_DIM(), 0.8)
                },
            );
        }
        // La connexion est déjà lue dans la pastille ronde : la ligne reste
        // sur le nom de la console et gagne la place pour l'afficher en entier.
        let mut detail = console_label(&identity)
            .map(|label| label.to_string())
            .unwrap_or_else(|| device.connection.label().to_string());
        if is_steam_puck_placeholder(device) {
            detail = lapinux_core::i18n::t("controllers.steam.puck_row").to_string();
        } else if matches!(power, PadPowerControl::On { pending: true, .. }) {
            detail =
                lapinux_core::i18n::t_args("controllers.ui.searching_ps", &[("detail", &detail)]);
        } else if matches!(power, PadPowerControl::On { failed: true, .. }) {
            detail = lapinux_core::i18n::t_args("controllers.ui.asleep_ps", &[("detail", &detail)]);
        } else if !device.connected {
            detail =
                lapinux_core::i18n::t_args("controllers.ui.offline_suffix", &[("detail", &detail)]);
        }
        let detail_font = egui::FontId::proportional(10.5);
        let detail = elide_to_width(painter, &detail, detail_font.clone(), text_width);
        let detail_width = painter
            .layout_no_wrap(detail.clone(), detail_font.clone(), TEXT_DIM())
            .size()
            .x;
        painter.text(
            egui::pos2(text_x, rect.top() + 32.0),
            egui::Align2::LEFT_TOP,
            detail,
            detail_font,
            TEXT_DIM(),
        );
        // Aperçu vivant des boutons enfoncés, sur la ligne du modèle, juste
        // après « Xbox 360 ». Il ne prend que la place laissée libre par le
        // texte et les pastilles de droite.
        if let Some(pad) = pad {
            paint_pressed_preview(
                painter,
                pad,
                identity.brand,
                pad_driver(&device.path).as_deref(),
                self.config.input.face_profile,
                text_x + detail_width + 7.0,
                rect.right() - 12.0 - right_space,
                rect.top() + 37.5,
            );
        }
        let right = rect.right() - 12.0;
        // Même gabarit que la pastille Bluetooth : un rond de 30 px, avec
        // six pixels entre le bouton et la pastille précédente.
        let power_rect = egui::Rect::from_center_size(
            egui::pos2(right - 15.0, rect.center().y),
            egui::vec2(30.0, 30.0),
        );
        let badge_right = if power_visible { right - 36.0 } else { right };
        if let Some(battery) = battery {
            paint_battery_ring(
                painter,
                egui::pos2(badge_right - 15.0, rect.center().y),
                30.0,
                battery,
                charging,
                time,
            );
        }
        let badge_center = badge_right
            - battery_width
            - if battery.is_some() { 8.0 } else { 0.0 }
            - connection_width / 2.0;
        connection_badge(
            painter,
            egui::pos2(badge_center, rect.center().y),
            device.connection,
            connection_width,
        );
        // La ligne entière capte le clic : le bouton d'alimentation est donc
        // peint et repéré dans sa zone, sans second widget qui se marcherait
        // dessus.
        let pointer_over_power = power_visible
            && ui.input(|input| {
                input
                    .pointer
                    .interact_pos()
                    .map_or(false, |pos| power_rect.contains(pos))
            });
        if power_visible {
            paint_pad_power_circled(
                ui,
                power_rect,
                power,
                response.hovered() && pointer_over_power,
            );
        }
        let keyboard = response.has_focus()
            && ui.input(|input| {
                input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
            });
        let clicked = response.clicked() || keyboard;
        let power_clicked = power_visible && response.clicked() && pointer_over_power;
        if pointer_over_power {
            response.on_hover_text(pad_power_hint(power));
        } else {
            response.on_hover_text(controller_tooltip(device, &identity, &shown));
        }
        ControllerRowAction {
            select: clicked && !pointer_over_power,
            power: power_clicked,
        }
    }

    /// Copies Steam Input conservées dans Lapinux pour cette manette. Le test
    /// reste volontairement local : il vérifie les entrées réelles de la
    /// manette, sans lancer Steam ni modifier ses fichiers.
    fn saved_steam_inputs_card(
        &mut self,
        ui: &mut egui::Ui,
        device: &DeviceInfo,
        pad: Option<&lapinux_input::PadState>,
    ) {
        let kind = steam_input_kind_for(device);
        let matching: Vec<SteamInputSaved> = self
            .saved_steam_inputs
            .iter()
            .filter(|saved| saved.controller_type == kind)
            .cloned()
            .collect();
        if matching.is_empty() {
            return;
        }
        let entries: Vec<(SteamInputSaved, String, String)> = matching
            .into_iter()
            .map(|saved| {
                let game = self.game_name(saved.appid);
                let source = if saved.source_appid != 0 && saved.source_appid != saved.appid {
                    self.game_name(saved.source_appid)
                } else {
                    String::new()
                };
                (saved, game, source)
            })
            .collect();
        let mut selected = self.pad_tests.steam_input_test;
        if selected.is_some()
            && !entries
                .iter()
                .any(|(saved, _, _)| Some(saved.id) == selected)
        {
            selected = None;
        }

        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(14.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t("ui.controllers_saved"))
                            .size(12.0)
                            .strong()
                            .color(TEXT()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t_fmt(
                                "controllers.kept",
                                &[entries.len().to_string()],
                            ))
                            .size(10.0)
                            .color(TEXT_DIM()),
                        );
                    });
                });
                ui.add_space(2.0);
                muted(ui, lapinux_core::i18n::t("controllers.vdf_hint"));
                ui.add_space(4.0);

                for (saved, game, source) in &entries {
                    let testing = selected == Some(saved.id);
                    egui::Frame::none()
                        .fill(PANEL())
                        .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                        .rounding(egui::Rounding::same(9.0))
                        .inner_margin(10.0)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(
                                            egui::RichText::new(&saved.title)
                                                .size(12.5)
                                                .strong()
                                                .color(TEXT()),
                                        );
                                        ui.label(
                                            egui::RichText::new("LAPINUX")
                                                .size(8.5)
                                                .strong()
                                                .color(OK),
                                        );
                                    });
                                    ui.horizontal_wrapped(|ui| {
                                        muted(ui, game);
                                        muted(ui, "·");
                                        muted(
                                            ui,
                                            lapinux_core::model::steam_input_label(
                                                &saved.controller_type,
                                            ),
                                        );
                                        if saved.saved_at > 0 {
                                            muted(ui, "·");
                                            muted(
                                                ui,
                                                lapinux_core::util::format_date(saved.saved_at),
                                            );
                                        }
                                    });
                                    if !source.is_empty() {
                                        muted(
                                            ui,
                                            lapinux_core::i18n::t_args(
                                                "controllers.ui.since",
                                                &[("source", &source)],
                                            ),
                                        );
                                    }
                                    muted(
                                        ui,
                                        lapinux_core::i18n::t_args(
                                            "steaminput.actions_count",
                                            &[(
                                                "count",
                                                &saved
                                                    .action_groups
                                                    .iter()
                                                    .map(|group| group.actions.len())
                                                    .sum::<usize>()
                                                    .to_string(),
                                            )],
                                        ),
                                    );
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if secondary_button(
                                            ui,
                                            if testing {
                                                lapinux_core::i18n::t("controllers.ui.close")
                                            } else {
                                                lapinux_core::i18n::t("controllers.ui.test")
                                            },
                                        )
                                        .on_hover_text(lapinux_core::i18n::t(
                                            "controllers.open_test",
                                        ))
                                        .clicked()
                                        {
                                            selected = if testing { None } else { Some(saved.id) };
                                        }
                                    },
                                );
                            });
                            ui.add_space(3.0);
                            let file = saved
                                .path
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("configuration.vdf");
                            ui.label(
                                egui::RichText::new(lapinux_core::i18n::t_args(
                                    "ui.copy_file",
                                    &[("file", &file.to_string())],
                                ))
                                .size(9.0)
                                .monospace()
                                .color(TEXT_DIM()),
                            )
                            .on_hover_text(saved.path.display().to_string());
                            egui::CollapsingHeader::new(lapinux_core::i18n::t(
                                "steaminput.profile_actions",
                            ))
                            .id_salt(("controllers-saved-actions", &saved.path))
                            .show(ui, |ui| {
                                crate::steaminput::saved_action_details(ui, saved);
                            });

                            if testing {
                                ui.add_space(7.0);
                                ui.separator();
                                ui.add_space(7.0);
                                ui.label(
                                    egui::RichText::new(lapinux_core::i18n::t(
                                        "ui.controllers_live",
                                    ))
                                    .size(10.5)
                                    .strong()
                                    .color(ACCENT()),
                                );
                                match pad {
                                    Some(pad) => {
                                        let pressed = if pad.pressed.is_empty() {
                                            lapinux_core::i18n::t("controllers.ui.no_button")
                                                .to_string()
                                        } else {
                                            pad.pressed.join(" · ")
                                        };
                                        ui.add_space(3.0);
                                        ui.label(
                                            egui::RichText::new(lapinux_core::i18n::t_args(
                                                "controllers.ui.buttons",
                                                &[("pressed", &pressed)],
                                            ))
                                            .size(10.5)
                                            .color(TEXT()),
                                        );
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "Stick G  {:.2} / {:.2}     Stick D  {:.2} / {:.2}",
                                                pad.axis("left_x"),
                                                pad.axis("left_y"),
                                                pad.axis("right_x"),
                                                pad.axis("right_y"),
                                            ))
                                            .size(10.0)
                                            .monospace()
                                            .color(TEXT_DIM()),
                                        );
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "{}  L {:.0} %     R {:.0} %",
                                                lapinux_core::i18n::t("controllers.triggers"),
                                                pad.axis("left_trigger") * 100.0,
                                                pad.axis("right_trigger") * 100.0,
                                            ))
                                            .size(10.0)
                                            .monospace()
                                            .color(TEXT_DIM()),
                                        );
                                        ui.add_space(3.0);
                                        muted(ui, lapinux_core::i18n::t("controllers.live_hint"));
                                    }
                                    None => {
                                        muted(ui, lapinux_core::i18n::t("controllers.ui.plug_in"));
                                    }
                                }
                                muted(ui, lapinux_core::i18n::t("controllers.ui.vdf_note"));
                            }
                        });
                    ui.add_space(6.0);
                }
            });

        self.pad_tests.steam_input_test = selected;
    }

    /// Détail de la manette sélectionnée, puis son test en direct.
    fn controller_detail(
        &mut self,
        ui: &mut egui::Ui,
        device: &DeviceInfo,
        pad: Option<&lapinux_input::PadState>,
    ) {
        let identity = pad_identity(&device);
        let key = controller_key(device);
        let face_profile = self.config.input.face_profile;
        let shown = pad_shown_name(&self.config.input.pad_names, device, &identity);
        let hide_dongle_axes = pad_setting(&self.config.input.pad_hide_dongle_axes, device)
            .copied()
            .unwrap_or(false);
        let mut hide_dongle_axes_edit = hide_dongle_axes;
        // La case s'affiche dès que des entrées sont masquées, même si la
        // liaison a changé depuis (dongle débranché, manette passée en USB ou
        // en Bluetooth) : sinon le réglage resterait actif sans aucun moyen de
        // le défaire, et la fiche semblerait incomplète pour toujours.
        let dongle_axes_toggle = hide_dongle_axes
            || (device.connection == DeviceLink::Dongle
                && (has_sticks(device) || has_triggers(device) || device.force_feedback));
        let mut features = controller_features(device, &identity, pad);
        if hide_dongle_axes {
            hide_dongle_features(&mut features);
        }
        let editing = self
            .pad_rename
            .as_ref()
            .map_or(false, |rename| rename.key == key);
        let mut rename_text = self
            .pad_rename
            .as_ref()
            .filter(|rename| rename.key == key)
            .map(|rename| rename.text.clone())
            .unwrap_or_else(|| shown.clone());
        let request_focus = self
            .pad_rename
            .as_ref()
            .map_or(false, |rename| rename.key == key && rename.focus);
        let (power_pending, power_armed, power_failed) = self
            .pad_power
            .as_ref()
            .filter(|power| power.key == key)
            .map_or((false, false, false), |power| {
                (
                    power.rx.is_some(),
                    power.kind == PowerKind::Off
                        && power
                            .armed_until
                            .map_or(false, |until| until > Instant::now()),
                    power.kind == PowerKind::On
                        && power.rx.is_none()
                        && power
                            .armed_until
                            .map_or(false, |until| until > Instant::now()),
                )
            });
        let power = if !device.connected {
            if device.address.is_some() {
                PadPowerControl::On {
                    pending: power_pending,
                    failed: power_failed,
                }
            } else {
                PadPowerControl::Hidden
            }
        } else if device.connection == DeviceLink::Bluetooth {
            PadPowerControl::Off {
                armed: power_armed,
                pending: power_pending,
            }
        } else {
            PadPowerControl::Wired
        };
        let mut action = PadHeaderAction::default();

        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let total = ui.available_width();
                let power_width = 74.0;
                let gap = 22.0;
                let wide = total >= 820.0;
                if wide {
                    let features_width = (total * 0.42).clamp(300.0, 430.0);
                    let left_width = (total - features_width - power_width - gap * 2.0).max(280.0);
                    ui.horizontal_top(|ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(left_width, 0.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(left_width);
                                action = controller_identity(
                                    ui,
                                    device,
                                    &identity,
                                    &shown,
                                    editing,
                                    request_focus,
                                    power,
                                    &mut rename_text,
                                );
                                ui.add_space(14.0);
                                controller_metadata(ui, device);
                            },
                        );
                        ui.add_space(gap);
                        ui.allocate_ui_with_layout(
                            egui::vec2(features_width, 0.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(features_width);
                                controller_feature_list(
                                    ui,
                                    &features,
                                    dongle_axes_toggle,
                                    &mut hide_dongle_axes_edit,
                                );
                            },
                        );
                        ui.add_space(gap);
                        ui.allocate_ui_with_layout(
                            egui::vec2(power_width, 0.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(power_width);
                                controller_power(ui, device, pad);
                            },
                        );
                    });
                } else {
                    ui.horizontal_top(|ui| {
                        let identity_width = (total - power_width - 10.0).max(200.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(identity_width, 0.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(identity_width);
                                action = controller_identity(
                                    ui,
                                    device,
                                    &identity,
                                    &shown,
                                    editing,
                                    request_focus,
                                    power,
                                    &mut rename_text,
                                );
                            },
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                            ui.allocate_ui_with_layout(
                                egui::vec2(power_width, 0.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    ui.set_width(power_width);
                                    controller_power(ui, device, pad);
                                },
                            );
                        });
                    });
                    ui.add_space(12.0);
                    controller_feature_list(
                        ui,
                        &features,
                        dongle_axes_toggle,
                        &mut hide_dongle_axes_edit,
                    );
                    ui.add_space(12.0);
                    controller_metadata(ui, device);
                }
            });

        if is_steam_controller_2026(device) {
            ui.add_space(10.0);
            self.steam_lizard_switch(ui);
        }

        ui.add_space(12.0);
        self.saved_steam_inputs_card(ui, device, pad);

        if hide_dongle_axes_edit != hide_dongle_axes {
            let mut config = self.config.clone();
            if hide_dongle_axes_edit {
                config.input.pad_hide_dongle_axes.insert(key.clone(), true);
            } else {
                config.input.pad_hide_dongle_axes.remove(&key);
            }
            if let Some(legacy) = legacy_path_key(device) {
                config.input.pad_hide_dongle_axes.remove(&legacy);
            }
            if self.apply_config(config) {
                self.notify(if hide_dongle_axes_edit {
                    lapinux_core::i18n::t("controllers.ui.hidden_entries")
                } else {
                    lapinux_core::i18n::t("controllers.ui.entries_restored")
                });
            }
        }

        if action.cancel {
            self.pad_rename = None;
        }
        if action.start_rename {
            self.pad_rename = Some(PadRename {
                key: key.clone(),
                text: shown.clone(),
                focus: true,
            });
        }
        if action.power {
            self.request_pad_power(device, ui.ctx());
        }
        if action.save {
            let mut config = self.config.clone();
            let trimmed = rename_text.trim();
            if trimmed.is_empty() {
                config.input.pad_names.remove(&key);
            } else {
                config
                    .input
                    .pad_names
                    .insert(key.clone(), trimmed.to_string());
            }
            if let Some(legacy) = legacy_path_key(device) {
                config.input.pad_names.remove(&legacy);
            }
            if self.apply_config(config) {
                self.notify(if trimmed.is_empty() {
                    lapinux_core::i18n::t("controllers.ui.name_restored").to_string()
                } else {
                    lapinux_core::i18n::t_args("controllers.ui.renamed", &[("name", &trimmed)])
                });
                self.pad_rename = None;
            }
        } else if let Some(rename) = self.pad_rename.as_mut().filter(|r| r.key == key) {
            // La saisie vit dans l'état : sans cette recopie, le texte tapé
            // serait remplacé par l'ancien nom à l'image suivante.
            rename.text = rename_text.clone();
            if request_focus {
                rename.focus = false;
            }
        }

        // Le noyau refuse la manette USB tant que la liaison Bluetooth est
        // active (même adresse). Le câble est pourtant visible : on propose
        // la seule bascule possible sans pilote noyau, en deux temps.
        let wired_pending = device.connected
            && device.connection == DeviceLink::Bluetooth
            && device.address.as_deref().map_or(false, |mac| {
                lapinux_input::usb_hid_for(mac).map_or(false, |usb| !usb.bound)
            });
        if wired_pending {
            ui.add_space(10.0);
            let pending = self
                .pad_power
                .as_ref()
                .map_or(false, |power| power.key == key && power.rx.is_some());
            egui::Frame::none()
                .fill(with_alpha(WARN, 0.08))
                .stroke(egui::Stroke::new(1.0_f32, with_alpha(WARN, 0.35)))
                .rounding(egui::Rounding::same(10.0))
                .inner_margin(12.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t("controllers.ui.usb_cable"))
                                .strong()
                                .color(WARN),
                        );
                        muted(ui, lapinux_core::i18n::t("controllers.ui.kernel_bluetooth"));
                        if ui
                            .add_enabled(
                                !pending,
                                egui::Button::new(if pending {
                                    lapinux_core::i18n::t("controllers.ui.switching")
                                } else {
                                    lapinux_core::i18n::t("controllers.ui.switch_wired")
                                }),
                            )
                            .clicked()
                        {
                            self.request_wired_handoff(device, ui.ctx());
                        }
                    });
                    ui.add_space(3.0);
                    muted(
                        ui,
                        lapinux_core::i18n::t("controllers.ui.switch_wired_hint"),
                    );
                });
        }

        ui.add_space(12.0);
        if device.connected {
            controller_test_panel(
                ui,
                device,
                pad,
                identity.brand,
                face_profile,
                identity.family,
                &features,
                &mut self.pad_tests,
                self.pad_audio.as_ref(),
            );
            if let Some(audio) = self.pad_audio.as_mut() {
                let wanted = self.pad_tests.mic_monitor;
                if let Err(error) = audio.sync_monitor(wanted) {
                    self.pad_tests.mic_monitor = false;
                    self.pad_tests.mic_note = Some(lapinux_core::i18n::t_args(
                        "controllers.ui.listen_failed",
                        &[("error", &error.to_string())],
                    ));
                }
            }
        } else {
            egui::Frame::none()
                .fill(CARD())
                .stroke(egui::Stroke::new(1.0_f32, with_alpha(WARN, 0.4)))
                .rounding(egui::Rounding::same(12.0))
                .inner_margin(14.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t("controllers.ui.paired_offline"))
                            .strong()
                            .color(WARN),
                    );
                    ui.add_space(4.0);
                    muted(
                        ui,
                        lapinux_core::i18n::t("controllers.ui.paired_offline_hint"),
                    );
                });
        }

        ui.add_space(12.0);
        if controller_information(ui, device, pad, &shown, &mut self.pad_info_open) {
            self.notify(lapinux_core::i18n::t("controllers.ui.info_copied").to_string());
        }
    }

    /// Relit le résultat de l'installation udev lancée depuis la fiche.
    fn poll_steam_lizard_install(&mut self) {
        let received = self.steam_lizard_install.as_ref().map(|rx| rx.try_recv());
        match received {
            Some(Ok(Ok(()))) => {
                self.steam_lizard_install = None;
                self.steam_lizard_writable = lapinux_input::device::steam_lizard_writable();
                self.steam_lizard_note = None;
                self.notify(if self.steam_lizard_writable {
                    lapinux_core::i18n::t("controllers.steam.install_success").to_string()
                } else {
                    lapinux_core::i18n::t("controllers.steam.install_partial").to_string()
                });
            }
            Some(Ok(Err(error))) => {
                self.steam_lizard_install = None;
                self.steam_lizard_note = Some(error.clone());
                self.notify(lapinux_core::i18n::t_args(
                    "controllers.steam.install_failed",
                    &[("error", &error)],
                ));
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.steam_lizard_install = None;
                self.steam_lizard_note = Some(
                    lapinux_core::i18n::t("controllers.steam.install_interrupted").to_string(),
                );
            }
            _ => {}
        }
    }

    /// Relit le résultat de l'installation de la configuration modprobe.
    fn poll_steam_lizard_persist(&mut self) {
        let received = self.steam_lizard_persist.as_ref().map(|rx| rx.try_recv());
        match received {
            Some(Ok(Ok(()))) => {
                self.steam_lizard_persist = None;
                self.steam_lizard_persistent =
                    lapinux_input::device::steam_lizard_config_installed();
                self.steam_lizard_note = None;
                self.notify(if self.steam_lizard_persistent {
                    lapinux_core::i18n::t("controllers.steam.persist_success").to_string()
                } else {
                    lapinux_core::i18n::t("controllers.steam.persist_partial").to_string()
                });
            }
            Some(Ok(Err(error))) => {
                self.steam_lizard_persist = None;
                self.steam_lizard_note = Some(error.clone());
                self.notify(lapinux_core::i18n::t_args(
                    "controllers.steam.install_failed",
                    &[("error", &error)],
                ));
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.steam_lizard_persist = None;
                self.steam_lizard_note = Some(
                    lapinux_core::i18n::t("controllers.steam.install_interrupted").to_string(),
                );
            }
            _ => {}
        }
    }

    /// Interrupteur du mode clavier/souris de la Steam Controller 2026.
    /// Contrairement à la bannière, il reste visible quand le mode est déjà
    /// désactivé : c'est le réglage de la manette, pas seulement une alerte.
    fn steam_lizard_switch(&mut self, ui: &mut egui::Ui) {
        let mode = self.steam_lizard_mode;
        let writable = self.steam_lizard_writable && mode.is_some();
        let installing = self.steam_lizard_install.is_some();
        let persisting = self.steam_lizard_persist.is_some();
        let busy = installing || persisting;
        let mut enabled = mode.unwrap_or(false);
        let mut changed = false;
        let mut install_requested = false;
        let mut persist_requested = false;
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(14.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t(
                                "controllers.steam.mode_title",
                            ))
                            .size(12.0)
                            .strong()
                            .color(TEXT()),
                        );
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t(
                                "controllers.steam.mode_desc",
                            ))
                            .size(10.5)
                            .color(TEXT_DIM()),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let response = ui
                            .add_enabled_ui(writable, |ui| switch(ui, &mut enabled))
                            .inner;
                        if response.changed() {
                            changed = true;
                        }
                        response.on_hover_text(if mode.is_none() {
                            lapinux_core::i18n::t("controllers.steam.mode_no_field")
                        } else if !self.steam_lizard_writable {
                            lapinux_core::i18n::t("controllers.steam.mode_install_hint")
                        } else {
                            lapinux_core::i18n::t("controllers.steam.mode_toggle_hint")
                        });
                    });
                });
                ui.add_space(5.0);
                let (text, color) = match mode {
                    Some(true) => (lapinux_core::i18n::t("controllers.steam.mode_active"), WARN),
                    Some(false) => (lapinux_core::i18n::t("controllers.steam.mode_native"), OK),
                    None => (
                        lapinux_core::i18n::t("controllers.steam.mode_unreadable"),
                        WARN,
                    ),
                };
                ui.label(egui::RichText::new(text).size(10.5).color(color));
                if !self.steam_lizard_writable && mode.is_some() {
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        muted(
                            ui,
                            lapinux_core::i18n::t("controllers.steam.switch_disabled"),
                        );
                        if ui
                            .add_enabled(
                                !busy,
                                egui::Button::new(if installing {
                                    lapinux_core::i18n::t("controllers.steam.installing_auth")
                                } else {
                                    lapinux_core::i18n::t("controllers.steam.install_rule")
                                }),
                            )
                            .on_hover_text(lapinux_core::i18n::t(
                                "controllers.steam.install_rule_hover",
                            ))
                            .clicked()
                        {
                            install_requested = true;
                        }
                    });
                    if installing {
                        muted(ui, lapinux_core::i18n::t("controllers.steam.auth_pending"));
                    }
                }
                if let Some(note) = self.steam_lizard_note.as_deref() {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(note).size(10.5).color(WARN));
                }
                ui.add_space(6.0);
                if self.steam_lizard_persistent {
                    muted(ui, lapinux_core::i18n::t("controllers.steam.persistent"));
                } else {
                    ui.horizontal_wrapped(|ui| {
                        muted(
                            ui,
                            lapinux_core::i18n::t("controllers.steam.not_persistent"),
                        );
                        if ui
                            .add_enabled(
                                !busy,
                                egui::Button::new(if persisting {
                                    lapinux_core::i18n::t("controllers.steam.installing")
                                } else {
                                    lapinux_core::i18n::t("controllers.steam.make_persistent")
                                }),
                            )
                            .on_hover_text(lapinux_core::i18n::t("controllers.steam.persist_hover"))
                            .clicked()
                        {
                            persist_requested = true;
                        }
                    });
                    if persisting {
                        muted(ui, lapinux_core::i18n::t("controllers.steam.auth_pending"));
                    }
                }
            });
        if persist_requested {
            let (tx, rx) = mpsc::channel();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let result = lapinux_input::device::install_steam_lizard_config();
                let _ = tx.send(result);
                ctx.request_repaint();
            });
            self.steam_lizard_persist = Some(rx);
            self.steam_lizard_note = None;
        }
        if install_requested {
            let (tx, rx) = mpsc::channel();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let result = lapinux_input::device::install_steam_lizard_rule();
                let _ = tx.send(result);
                ctx.request_repaint();
            });
            self.steam_lizard_install = Some(rx);
            self.steam_lizard_note = None;
        }
        if changed {
            match lapinux_input::device::set_steam_lizard_mode(enabled) {
                Ok(()) => {
                    self.steam_lizard_mode = Some(enabled);
                    self.steam_lizard_note = None;
                    self.notify(if enabled {
                        lapinux_core::i18n::t("controllers.steam.note_enabled").to_string()
                    } else {
                        lapinux_core::i18n::t("controllers.steam.note_disabled").to_string()
                    });
                }
                Err(error) => {
                    self.steam_lizard_note = Some(error.clone());
                    self.notify(lapinux_core::i18n::t_args(
                        "controllers.steam.note_failed",
                        &[("error", &error)],
                    ));
                }
            }
        }
    }

    /// Colonne de droite quand aucune manette n'est connue.
    fn controllers_empty(&self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(egui::Margin::symmetric(24.0, 34.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    let (icon, _) =
                        ui.allocate_exact_size(egui::vec2(46.0, 46.0), egui::Sense::hover());
                    ui.painter().text(
                        icon.center(),
                        egui::Align2::CENTER_CENTER,
                        nav_glyph(Page::Controllers),
                        icon_font(42.0),
                        TEXT_DIM(),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t("controllers.empty"))
                            .font(display_font(15.0))
                            .color(TEXT()),
                    );
                    ui.add_space(4.0);
                    muted(ui, lapinux_core::i18n::t("controllers.empty_hint"));
                });
            });
    }
}

/// Entrée synthétique du Puck : elle donne une place stable dans « Appairées »
/// tant que hid-steam n'expose pas encore de gamepad natif.
pub(super) fn steam_puck_placeholder() -> DeviceInfo {
    DeviceInfo {
        path: PathBuf::new(),
        name: lapinux_core::i18n::t("controllers.steam.device_name").to_string(),
        vendor: 0x28de,
        product: 0x1304,
        gamepad: true,
        buttons: 0,
        button_names: Vec::new(),
        axes: Vec::new(),
        force_feedback: false,
        touchpad: true,
        connection: DeviceLink::Dongle,
        battery: None,
        battery_status: None,
        address: None,
        connected: false,
        steam_bridge: false,
        udev: lapinux_core::model::UdevIdentity {
            serial: Some("lapinux-steam-puck".into()),
            ..Default::default()
        },
    }
}

pub(super) fn is_steam_puck_placeholder(device: &DeviceInfo) -> bool {
    device.udev.serial.as_deref() == Some("lapinux-steam-puck")
}

/// Vrai si le Puck doit apparaître dans « Appairées » : présent, sans pont
/// actif, et sans manette Steam déjà exposée par le noyau ou BlueZ.
pub(super) fn steam_puck_needs_entry(
    present: bool,
    bridge_running: bool,
    gamepads: &[DeviceInfo],
) -> bool {
    present
        && !bridge_running
        && !gamepads.iter().any(|device| {
            device.gamepad
                && lapinux_core::model::is_steam_controller_2026(device.vendor, device.product)
        })
}
