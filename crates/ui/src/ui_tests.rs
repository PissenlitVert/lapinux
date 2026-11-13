use super::*;

/// Corps d'une fonction de premier niveau, jusqu'à son accolade fermante.
fn top_level_function<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("fonction absente : {signature}"));
    let rest = &source[start..];
    let end = rest
        .find("\n}\n")
        .unwrap_or_else(|| panic!("fin de fonction absente : {signature}"));
    &rest[..end + 3]
}

/// La fiche de réglages d'un jeu ne revient jamais aux cases à cocher ni aux
/// curseurs egui sans piste : elle utilise le switch et le slider dessiné,
/// comme le menu Réglages global. Le test vérifie aussi que les helpers
/// réellement appelés dessinent une piste remplie, une valeur et les bornes.
/// Un anti-triche coupe les outils du jeu, et les rallumer passe par un
/// avertissement : chaque contrôle protégé porte son étiquette.
#[test]
fn un_jeu_protege_coupe_les_outils_et_avertit_avant_reactivation() {
    let sheet = include_str!("game_sheet.rs");
    assert!(sheet.contains("fn protected_bool("));
    for tag in ["shim", "vkbasalt", "hud", "record", "afmf", "injections"] {
        assert!(
            sheet.contains(&format!(":{tag}\"")),
            "outil sans protection : {tag}"
        );
    }
    for key in [
        "game_settings.anticheat_banner",
        "game_settings.anticheat_default_off",
        "game_settings.anticheat_confirm",
        "game_settings.anticheat_enabled",
    ] {
        assert!(sheet.contains(key), "avertissement manquant : {key}");
    }
}

#[test]
fn game_settings_screen_has_no_tick_boxes_or_trackless_sliders() {
    for (name, source) in [
        ("game_sheet.rs", include_str!("game_sheet.rs")),
        ("game_contents.rs", include_str!("game_contents.rs")),
        ("sheet_saves.rs", include_str!("sheet_saves.rs")),
        ("configurations.rs", include_str!("configurations.rs")),
    ] {
        for legacy in [
            ".checkbox(",
            "egui::Slider",
            "Slider::new",
            "optional_bool_combo",
            "TICK",
        ] {
            assert!(
                !source.contains(legacy),
                "contrôle legacy {legacy:?} encore présent dans {name}"
            );
        }
    }

    let sheet = include_str!("game_sheet.rs");
    let form = &sheet[sheet
        .find("pub(super) fn game_settings_form")
        .expect("formulaire des réglages jeu")..];
    for tab in [
        "GameSettingsTab::Launch",
        "GameSettingsTab::InGame",
        "GameSettingsTab::Display",
        "GameSettingsTab::Graphics",
        "GameSettingsTab::Captures",
        "GameSettingsTab::Saves",
    ] {
        assert!(form.contains(tab), "onglet absent du formulaire : {tab}");
    }
    assert!(form.contains("game_override_bool("));
    assert!(form.contains("game_override_slider("));
    assert!(form.contains("switch("));

    // Chaque booléen par jeu garde son contrôle dans cet écran, y compris les
    // sauvegardes cloud dessinées par l'onglet Sauvegardes.
    let screen = format!(
        "{}{}{}",
        include_str!("game_sheet.rs"),
        include_str!("game_contents.rs"),
        include_str!("sheet_saves.rs")
    );
    for field in [
        "gamescope",
        "vkbasalt",
        "hdr",
        "adaptive_sync",
        "hud_on_screen",
        "record_badge",
        "record_on_launch",
        "stop_on_exit",
        "record_audio",
        "record_microphone",
        "saves_auto",
        "saves_skip_cloud",
        "saves_relaunch",
        "grab_devices",
        "cloud_sync",
        "steam_shim",
    ] {
        assert!(
            screen.contains(&format!("settings.{field}")),
            "booléen par jeu non câblé dans l'écran : {field}"
        );
    }

    let bool_override = top_level_function(sheet, "fn game_override_bool(");
    assert!(bool_override.contains("switch(ui, &mut current)"));
    assert!(bool_override.contains("game_settings.reset_default"));
    assert!(bool_override.contains("inherited_bool(global)"));

    let slider_override = top_level_function(sheet, "fn game_override_slider<");
    assert!(slider_override.contains("settings_slider(ui, &mut current"));
    assert!(slider_override.contains("game_settings.reset_default"));
    assert!(slider_override.contains("game_settings.default_global"));

    let lib = include_str!("lib.rs");
    let toggle = top_level_function(lib, "fn switch(");
    assert!(toggle.contains("painter.rect_filled(rect"));
    assert!(toggle.contains("painter.circle_filled("));

    let slider = top_level_function(lib, "fn settings_slider<");
    assert!(slider.contains("let track = egui::Rect::from_min_max"));
    assert!(slider.contains("painter.rect_filled(track"));
    assert!(slider.contains("painter.circle_filled("));
    assert!(slider.contains("format!(\"{number}{suffix}\")"));
    assert!(slider.contains("format_slider_bound(min)"));
    assert!(slider.contains("format_slider_bound(max)"));
}

/// L'en-tête additionne les poids mesurés et dit où en est la mesure.
#[test]
fn the_header_sums_the_measured_protons() {
    let proton = |name: &str| ProtonInfo {
        name: name.to_string(),
        path: std::path::PathBuf::from(format!("/protons/{name}")),
        source: "compatibilitytools".to_string(),
        version: String::new(),
        runnable: true,
    };
    let protons = vec![proton("A"), proton("B")];
    let mut sizes: HashMap<PathBuf, u64> = HashMap::new();
    assert_eq!(proton_weight_line(&[], &sizes), "");
    assert!(proton_weight_line(&protons, &sizes)
        .contains(lapinux_core::i18n::t("protons.weight_measuring")));
    sizes.insert(protons[0].path.clone(), 1_500_000_000);
    let partial = proton_weight_line(&protons, &sizes);
    assert_eq!(
        partial,
        lapinux_core::i18n::t_args(
            "protons.weight_partial",
            &[
                ("weight", &lapinux_core::util::format_bytes(1_500_000_000)),
                ("measured", "1"),
                ("builds", "2"),
            ],
        )
    );
    sizes.insert(protons[1].path.clone(), 500_000_000);
    let complete = proton_weight_line(&protons, &sizes);
    assert_eq!(
        complete,
        lapinux_core::i18n::t_args(
            "protons.weight_total",
            &[
                ("weight", &lapinux_core::util::format_bytes(2_000_000_000)),
                ("builds", "2"),
            ],
        )
    );
}

/// Le poids d'un Proton se mesure une fois, hors de l'image, et disparaît
/// de la table quand le build quitte la liste.
#[test]
fn proton_sizes_are_measured_then_forgotten() {
    let root = std::env::temp_dir().join(format!("lapinux-proton-size-{}", std::process::id()));
    let build = root.join("GE-Proton11-7");
    std::fs::create_dir_all(build.join("files/bin")).unwrap();
    std::fs::write(build.join("proton"), vec![0u8; 1024]).unwrap();
    std::fs::write(build.join("files/bin/wine"), vec![0u8; 2048]).unwrap();
    let protons = vec![ProtonInfo {
        name: "GE-Proton11-7".into(),
        path: build.clone(),
        source: "compatibilitytools".into(),
        version: "11.0".into(),
        runnable: true,
    }];
    let ctx = egui::Context::default();
    let mut sizes = ProtonSizes::default();
    sizes.ensure(&ctx, &protons);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        sizes.poll();
        if sizes.get(&build).is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(sizes.get(&build), Some(3072), "poids des deux fichiers");
    // Build supprimé : sa taille ne reste pas dans la table.
    sizes.ensure(&ctx, &[]);
    assert_eq!(sizes.get(&build), None);
    let _ = std::fs::remove_dir_all(&root);
}

/// Une image en paysage, comme les visuels de boutique.
fn banner_png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(width, height, image::Rgba([40, 80, 120, 255]));
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .expect("PNG écrit");
    out
}

#[test]
fn a_gif_cover_decodes_like_the_other_formats() {
    // itch.io publie certaines couvertures en GIF (animé ou non) : sans
    // décodeur GIF, la tuile restait une lettre.
    const GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;";
    assert!(image::load_from_memory(GIF).is_ok(), "GIF non décodé");
}

#[test]
fn an_itch_banner_becomes_a_portrait_tile() {
    // Un bandeau 4:3 d'itch, centré sur un fond de la même image : la
    // tuile prend le format 2:3 de la grille.
    let bytes = portrait_store_cover(&banner_png(40, 30)).expect("couverture itch acceptée");
    let image = image::load_from_memory(&bytes).expect("image écrite");
    assert_eq!(image.width(), 40);
    assert_eq!(image.height(), 60, "format 2:3");
}

#[test]
fn a_store_banner_can_fill_a_missing_cover() {
    let bytes = portrait_store_cover(&banner_png(40, 30)).unwrap();
    let image = image::load_from_memory(&bytes).unwrap();
    assert_eq!((image.width(), image.height()), (40, 60));
}

#[test]
fn steam_art_defaults_complete_missing_fields_without_replacing_official_urls() {
    let details = SteamDetails {
        hero_image: "https://official.test/banner".into(),
        ..Default::default()
    };
    let complete = steam_art_defaults(123, &details);
    assert_eq!(complete.hero_image, details.hero_image);
    assert!(complete.header_image.ends_with("/123/header.jpg"));
    assert!(complete.capsule_image.ends_with("/123/capsule_231x87.jpg"));
    assert!(steam_art_defaults(0, &SteamDetails::default())
        .header_image
        .is_empty());
}

#[test]
fn a_game_icon_does_not_fall_back_to_the_store_logo() {
    let details = SteamDetails {
        icon_image: String::new(),
        logo_image: "https://example.test/logo".into(),
        capsule_image: "https://example.test/capsule".into(),
        header_image: String::new(),
        ..Default::default()
    };
    assert!(
        SteamArt::Icon.urls(&details).is_empty(),
        "le logo, la capsule et la bannière ne doivent pas devenir l'icône du jeu"
    );
    let with_icon = SteamDetails {
        icon_image: "https://example.test/icon".into(),
        ..details
    };
    assert_eq!(
        SteamArt::Icon.urls(&with_icon),
        vec!["https://example.test/icon".to_string()]
    );
}

#[test]
fn store_art_uses_each_format_and_falls_back_to_same_game() {
    for store in [
        lapinux_core::stores::Store::Epic,
        lapinux_core::stores::Store::Gog,
        lapinux_core::stores::Store::Itch,
    ] {
        let details = lapinux_core::stores::StoreDetails {
            store,
            icon: "https://example.test/icon".into(),
            capsule: "https://example.test/capsule".into(),
            banner: "//example.test/banner".into(),
            cover: "https://example.test/cover".into(),
            ..Default::default()
        };
        let images = store_art_candidates(Some(&details), "https://example.test/library");
        assert_eq!(images[0][0], "https://example.test/icon");
        assert_eq!(images[1][0], "https://example.test/capsule");
        assert_eq!(images[2][0], "https://example.test/banner");
        assert_eq!(images[3][0], "https://example.test/cover");
        let only_cover = store_art_candidates(None, "https://example.test/library");
        assert!(only_cover
            .iter()
            .all(|urls| urls == &["https://example.test/library"]));
        assert!(store_art_candidates(None, "").iter().all(Vec::is_empty));
    }
}

#[test]
fn store_art_only_marks_the_formats_that_are_really_missing() {
    let details = lapinux_core::stores::StoreDetails {
        icon: String::new(),
        capsule: "https://example.test/wide".into(),
        banner: "https://example.test/wide".into(),
        cover: "https://example.test/cover".into(),
        ..Default::default()
    };
    // L'icône manque : la jaquette qui la remplace est un repli.
    assert!(store_art_fallback(Some(&details), 0, 1));
    // La capsule et la bannière partagent le même visuel publié : aucune
    // des deux n'est un repli.
    assert!(!store_art_fallback(Some(&details), 1, 0));
    assert!(!store_art_fallback(Some(&details), 2, 0));
    assert!(!store_art_fallback(Some(&details), 3, 0));
    // Sans fiche, la jaquette de la bibliothèque est un repli pour tout.
    assert!(store_art_fallback(None, 2, 0));
    // Si l'adresse publiée échoue, le candidat suivant est bien un repli.
    assert!(store_art_fallback(Some(&details), 2, 1));
}

#[test]
fn contents_show_published_missing_and_owned_extras() {
    // Steam publie trois contenus, le compte en possède un seul : la liste
    // doit garder les trois et dire lequel est au compte.
    let published = vec![
        lapinux_core::model::SteamDlcRef {
            appid: 10,
            name: "Costume".into(),
            owned: false,
            download: 154_000_000,
            install: 154_000_000,
            image: String::new(),
        },
        lapinux_core::model::SteamDlcRef {
            appid: 11,
            name: "Deluxe Kit".into(),
            owned: false,
            download: 960_000_000,
            install: 989_000_000,
            image: String::new(),
        },
        lapinux_core::model::SteamDlcRef {
            appid: 12,
            name: "Soundtrack".into(),
            owned: false,
            download: 0,
            install: 0,
            image: String::new(),
        },
    ];
    let owned = vec![
        SteamDlc {
            appid: 12,
            name: "Soundtrack".into(),
            download: 70_000_000,
            install: 80_000_000,
            installed: true,
        },
        // Contenu du compte absent de la liste publique du jeu.
        SteamDlc {
            appid: 13,
            name: "Artbook".into(),
            download: 5_000_000,
            install: 6_000_000,
            installed: false,
        },
    ];
    let rows = content_rows(&published, &owned);
    assert_eq!(rows.len(), 4, "les trois publiés et l'extra du compte");
    assert_eq!(rows.iter().filter(|row| row.owned).count(), 2);
    // Un contenu non possédé garde son nom et son poids.
    assert_eq!(rows[0].name, "Costume");
    assert!(!rows[0].owned);
    assert_eq!(rows[0].download, 154_000_000);
    // Le poids du catalogue complète une fiche muette.
    assert_eq!(rows[2].download, 70_000_000);
    assert!(rows[2].installed);
    assert!(rows[3].owned);
    assert_eq!(rows[3].name, "Artbook");
}

#[test]
fn a_game_that_is_not_installed_announces_its_installed_weight() {
    use lapinux_core::model::{SteamDetails, SteamSize};
    let published = SteamDetails {
        size_windows: Some(SteamSize {
            install: 5_000_000_000,
            download: 4_000_000_000,
        }),
        ..Default::default()
    };
    // C'est la place occupée une fois installé qui compte, pas le
    // téléchargement.
    assert_eq!(
        published_install_bytes(&published, "windows"),
        Some(5_000_000_000)
    );
    // Une plateforme sans poids propre retombe sur celle qui en publie.
    assert_eq!(
        published_install_bytes(&published, "linux"),
        Some(5_000_000_000)
    );
    // Un poids d'installation absent laisse celui du téléchargement.
    let only_download = SteamDetails {
        size_linux: Some(SteamSize {
            install: 0,
            download: 3_000,
        }),
        ..Default::default()
    };
    assert_eq!(
        published_install_bytes(&only_download, "linux"),
        Some(3_000)
    );
    // Rien de publié : rien d'annoncé, plutôt qu'un faux zéro.
    assert_eq!(
        published_install_bytes(&SteamDetails::default(), "linux"),
        None
    );
}

#[test]
fn the_playtime_tooltip_spells_out_each_platform() {
    use lapinux_core::model::SteamAccountPlay;
    let account = SteamAccountPlay {
        minutes_total: Some(500),
        // Une plateforme sans minute ne s'écrit pas.
        minutes_2weeks: Some(0),
        minutes_linux: Some(300),
        minutes_windows: Some(200),
        ..Default::default()
    };
    assert_eq!(
        playtime_tooltip(Some(&account), None),
        "Total Steam · 8 h 20\nLinux · 5 h 00\nWindows · 3 h 20"
    );
    // Sans lecture du compte, le manifeste local est nommé : un fichier ne
    // doit pas se faire passer pour un total Steam.
    assert_eq!(
        playtime_tooltip(None, Some(5400)),
        "Fichiers locaux · 1 h 30"
    );
    // Rien à dire : aucune infobulle.
    assert_eq!(playtime_tooltip(None, None), "");
}

#[test]
fn the_card_says_never_played_instead_of_nothing() {
    // 3 h 55, puis l'absence de partie, puis l'absence d'information :
    // trois cas distincts, trois textes distincts.
    assert_eq!(played_label(Some(235)), "3 h 55");
    assert_eq!(
        played_label(Some(0)),
        lapinux_core::i18n::t("library.never_played_game")
    );
    assert_eq!(played_label(None), "");
}

#[test]
fn a_panel_title_sits_at_the_middle_of_its_band() {
    // 620 de large moins les deux marges de 22 : le titre doit tomber au
    // centre du bandeau, croix comprise.
    let (left, middle, close) = panel_header_split(576.0);
    assert_eq!(left, close);
    assert!((left + middle / 2.0 - 288.0).abs() < 0.01);
    assert_eq!(left + middle + close, 576.0);
    // Un panneau étroit garde un titre lisible plutôt qu'une largeur
    // négative.
    let (_, narrow, _) = panel_header_split(100.0);
    assert!(narrow >= 120.0);
}

/// La corbeille du modal prend place juste avant la croix, sans décentrer le
/// titre : la marge gauche réserve la même largeur que l'action à droite.
#[test]
fn a_panel_header_action_stays_left_of_the_close_button() {
    let (left, middle, action, close) = panel_header_split_with_action(576.0, true);
    assert_eq!(action, 30.0);
    assert_eq!(close, 30.0);
    assert_eq!(left, action + close);
    assert!((left + middle / 2.0 - 288.0).abs() < 0.01);
    assert_eq!(left + middle + action + close, 576.0);
}

#[test]
fn a_family_version_lists_only_the_members_dlc_for_that_game() {
    use lapinux_core::model::{SteamDetails, SteamFamilyDlc};
    let details = SteamDetails {
        dlc_ids: vec![100, 200, 300],
        ..Default::default()
    };
    let family = vec![
        SteamFamilyDlc {
            appid: 100,
            name: "A".into(),
            owners: vec![2],
        },
        SteamFamilyDlc {
            appid: 200,
            name: "B".into(),
            owners: vec![3],
        },
        // Le même membre possède aussi un contenu d'un autre jeu : la fiche
        // de ce jeu ne doit pas le lui attribuer.
        SteamFamilyDlc {
            appid: 400,
            name: "Autre jeu".into(),
            owners: vec![2],
        },
    ];
    let dlc = crate::family_version_dlc(&details, &family, 2);
    assert_eq!(dlc.len(), 1);
    assert_eq!(dlc[0].appid, 100);
    assert_eq!(dlc[0].name, "A");
    assert_eq!(
        crate::family_version_dlc(&details, &family, 3)[0].appid,
        200
    );
    assert!(crate::family_version_dlc(&details, &family, 4).is_empty());
}

#[test]
fn a_family_version_panel_lists_only_the_members_contents() {
    // Le panneau d'une version familiale ne montre que ce que ce membre
    // possède : un DLC publié qu'il n'a pas ne peut pas être téléchargé et
    // faisait diverger la liste du compteur du bouton.
    let published = vec![
        lapinux_core::model::SteamDlcRef {
            appid: 10,
            name: "Costume".into(),
            owned: true,
            download: 10,
            install: 10,
            image: String::new(),
        },
        lapinux_core::model::SteamDlcRef {
            appid: 11,
            name: "Deluxe".into(),
            owned: false,
            download: 20,
            install: 20,
            image: String::new(),
        },
        lapinux_core::model::SteamDlcRef {
            appid: 12,
            name: "Soundtrack".into(),
            owned: true,
            download: 30,
            install: 30,
            image: String::new(),
        },
    ];
    let owned = vec![
        SteamDlc {
            appid: 11,
            name: "Deluxe".into(),
            download: 20,
            install: 20,
            installed: false,
        },
        SteamDlc {
            appid: 13,
            name: "Artbook".into(),
            download: 5,
            install: 5,
            installed: false,
        },
    ];
    // La version personnelle garde les contenus publiés qu'elle ne possède
    // pas ; la version familiale ne garde que la liste du membre.
    assert_eq!(content_rows_for_version(&published, &owned, false).len(), 4);
    let family = content_rows_for_version(&published, &owned, true);
    assert_eq!(
        family.iter().map(|row| row.appid).collect::<Vec<_>>(),
        vec![11, 13]
    );
    assert!(family.iter().all(|row| row.owned));
    assert_eq!(family[0].name, "Deluxe");
    assert_eq!(family[0].download, 20);
}

#[test]
fn contents_are_grouped_owned_first_and_preselected() {
    // Trois contenus dans l'ordre de Steam, deux possédés dont un sur le
    // disque : le panneau sépare les deux catégories et coche ce qui est au
    // compte.
    let mut rows = vec![
        row(10, "Costume", 154_000_000, false, false),
        row(11, "Deluxe Kit", 960_000_000, true, false),
        row(12, "Soundtrack", 70_000_000, true, true),
    ];
    contents_owned_first(&mut rows);
    let order: Vec<u32> = rows.iter().map(|row| row.appid).collect();
    // Le tri est stable : l'ordre de Steam survit dans chaque groupe.
    assert_eq!(order, vec![11, 12, 10]);

    // Rien n'a été coché à la main : ce que le compte possède est retenu.
    assert!(content_checked(None, rows[0].owned, rows[0].appid));
    assert!(content_checked(None, rows[1].owned, rows[1].appid));
    assert!(!content_checked(None, rows[2].owned, rows[2].appid));
    // Un clic remplace ce défaut, dans les deux sens.
    let mut uncheck = HashSet::new();
    uncheck.insert(10);
    assert!(!content_checked(Some(&uncheck), true, 11));
    assert!(content_checked(Some(&uncheck), false, 10));

    // Le bouton de téléchargement compte les contenus retenus, et le défaut
    // vaut tout ce que le compte possède.
    let owned = vec![
        SteamDlc {
            appid: 11,
            name: "Deluxe Kit".into(),
            download: 960_000_000,
            install: 989_000_000,
            installed: false,
        },
        SteamDlc {
            appid: 12,
            name: "Soundtrack".into(),
            download: 70_000_000,
            install: 80_000_000,
            installed: true,
        },
    ];
    let mut state = HashMap::new();
    let preset = selected_contents(&state, 3768760, &owned);
    assert_eq!(preset.len(), 2);
    assert_eq!(
        download_size(Some(1000), &owned, Some(&preset)),
        Some(960_001_000),
        "le contenu déjà sur le disque ne pèse plus"
    );
    state.insert(3768760, HashSet::new());
    let cleared = selected_contents(&state, 3768760, &owned);
    assert!(cleared.is_empty());
    assert_eq!(
        download_size(Some(1000), &owned, Some(&cleared)),
        Some(1000)
    );

    // Le poids à droite du nom dit ce que la ligne pèse vraiment.
    assert_eq!(content_weight_text(&rows[0]), "+ 960 Mo");
    assert_eq!(
        content_weight_text(&rows[1]),
        lapinux_core::i18n::t("game_contents.already_on_disk")
    );
    assert_eq!(content_weight_text(&rows[2]), "+ 154 Mo");
    // Ni possédé, ni dépôt publié : Steam ne dit rien.
    let unknown = row(14, "Weapon Skin", 0, false, false);
    assert_eq!(
        content_weight_text(&unknown),
        lapinux_core::i18n::t("app.unknown_size")
    );
    // Possédé sans dépôt à lui : livré avec le jeu, pas « inconnu ».
    let bundled = row(13, "Gadget Skin", 0, true, false);
    assert_eq!(
        content_weight_text(&bundled),
        lapinux_core::i18n::t("app.included_in_game")
    );
}

#[test]
fn contents_are_split_in_two_titled_sections() {
    let mut rows = vec![
        row(10, "Costume", 154_000_000, false, false),
        row(11, "Deluxe Kit", 960_000_000, true, false),
        row(12, "Soundtrack", 70_000_000, true, true),
    ];
    contents_owned_first(&mut rows);
    let sections = content_sections(&rows);
    assert_eq!(sections.len(), 2, "une section par catégorie");
    assert_eq!(
        sections[0].0,
        lapinux_core::i18n::t_args("app.owned_count", &[("owned_count", "2")])
    );
    assert_eq!(
        sections[0]
            .1
            .iter()
            .map(|row| row.appid)
            .collect::<Vec<_>>(),
        vec![11, 12]
    );
    assert_eq!(
        sections[1].0,
        lapinux_core::i18n::t_fmt("app.not_owned_count", &["1".to_string()])
    );
    assert_eq!(sections[1].1[0].appid, 10);
    // Un jeu dont tout est possédé n'a pas de section vide en dessous.
    assert_eq!(content_sections(&[row(20, "DLC", 0, true, false)]).len(), 1);

    // Le premier clic dans la liste part de ce que le panneau montre :
    // décocher une ligne laisse les autres cochées.
    let mut selection: HashSet<u32> = rows
        .iter()
        .filter(|row| row.owned)
        .map(|row| row.appid)
        .collect();
    selection.remove(&11);
    assert!(!content_checked(Some(&selection), true, 11));
    assert!(content_checked(Some(&selection), true, 12));
    assert!(!content_checked(Some(&selection), false, 10));
}

fn row(appid: u32, name: &str, download: u64, owned: bool, installed: bool) -> ContentRow {
    ContentRow {
        appid,
        name: name.into(),
        download,
        owned,
        installed,
        image: String::new(),
    }
}

#[test]
fn fallback_fonts_are_picked_one_per_pattern_in_priority_order() {
    let files = vec![
        PathBuf::from("/usr/share/fonts/dejavu/DejaVuSans.ttf"),
        PathBuf::from("/usr/share/fonts/symbola/Symbola.ttf"),
        PathBuf::from("/usr/share/fonts/noto/NotoSansSymbols2-Regular.ttf"),
    ];
    let read = |path: &Path| Some(path.display().to_string().into_bytes());
    let picked = pick_fallback_fonts(&files, read);
    let names: Vec<&str> = picked.iter().map(|(name, _)| name.as_str()).collect();
    // Les symboles passent avant les familles larges, et chaque motif ne
    // donne qu'une police : la liste reste courte.
    assert_eq!(names.len(), 3);
    assert!(names[0].ends_with("NotoSansSymbols2-Regular.ttf"));
    assert!(names[1].ends_with("Symbola.ttf"));
    assert!(names[2].ends_with("DejaVuSans.ttf"));
}

#[test]
fn an_unreadable_font_gives_its_place_to_the_next_of_its_pattern() {
    let files = vec![
        PathBuf::from("/usr/share/fonts/noto/NotoSansSymbols2-Regular.ttf"),
        PathBuf::from("/usr/share/fonts/noto/NotoSansSymbols[wght].ttf"),
    ];
    // Le premier est refusé (trop gros, illisible…) : le suivant du même
    // motif prend sa place au lieu de laisser un trou.
    let read =
        |path: &Path| (!path.to_string_lossy().contains("Symbols2")).then(|| b"police".to_vec());
    let picked = pick_fallback_fonts(&files, read);
    assert_eq!(picked.len(), 1);
    assert!(picked[0].0.ends_with("NotoSansSymbols[wght].ttf"));
}

#[test]
fn color_emoji_fonts_are_never_loaded() {
    // Le moteur de texte ne trace que des contours : une police COLR ou
    // bitmap ne sortirait rien du tout.
    assert!(!matches_fallback_pattern(
        Path::new("/usr/share/fonts/noto/NotoColorEmoji.ttf"),
        &["notoemoji", "noto"]
    ));
    assert!(!matches_fallback_pattern(
        Path::new("/usr/share/fonts/noto/Noto-COLRv1.ttf"),
        &["noto"]
    ));
    // La version monochrome, elle, est la bienvenue.
    assert!(matches_fallback_pattern(
        Path::new("/usr/share/fonts/noto/NotoEmoji-Regular.ttf"),
        &["notoemoji"]
    ));
}

#[test]
fn a_fallback_font_prefers_the_regular_weight() {
    let mut files = vec![
        PathBuf::from("/usr/share/fonts/noto/NotoSansCJK-Black.ttc"),
        PathBuf::from("/usr/share/fonts/noto/NotoSansCJK-Regular.ttc"),
    ];
    files.sort_by(|a, b| fallback_weight_rank(a).cmp(&fallback_weight_rank(b)));
    let read = |path: &Path| Some(path.display().to_string().into_bytes());
    let picked = pick_fallback_fonts(&files, read);
    assert_eq!(picked.len(), 1);
    // Un nom japonais ne s'affiche pas en très gras : Regular passe avant
    // Black malgré l'ordre alphabétique.
    assert!(picked[0].0.ends_with("NotoSansCJK-Regular.ttc"));
}

#[test]
fn a_font_is_verified_before_it_reaches_the_renderer() {
    // Le moteur de texte panique sur un fichier illisible : rien de tel ne
    // doit lui arriver depuis le disque.
    let inter = include_bytes!("../../../assets/fonts/Inter-Medium.ttf");
    assert!(readable_font(inter));
    assert!(!readable_font(b"pas une police"));
    assert!(!readable_font(&[]));
}

#[test]
fn gyro_readings_are_in_degrees_per_second() {
    // 4096 brut sur un rapport DualSense = 0,125 normalisé = 256 °/s.
    assert!((gyro_degrees(4096.0 / 32768.0) - 256.0).abs() < 0.01);
    assert!((gyro_degrees(0.01) - 20.48).abs() < 0.01);
    assert!((gyro_degrees(2.0) - GYRO_FULL_SCALE).abs() < 0.01);
    assert!((accel_in_g(0.25) - 1.0).abs() < 0.001);
}

#[test]
fn steam_qr_keeps_its_finders_and_quiet_zone() {
    let image = qr_image("https://s.team/q/1/123456789").expect("QR encodable");
    let side = image.size[0];
    let quiet = 4usize;
    let dark = |x: usize, y: usize| image.pixels[y * side + x] == egui::Color32::BLACK;
    // Marge blanche sur tout le pourtour : sans elle, un scanner échoue.
    for i in 0..side {
        for j in 0..quiet {
            assert!(!dark(i, j) && !dark(i, side - 1 - j));
            assert!(!dark(j, i) && !dark(side - 1 - j, i));
        }
    }
    // Les trois motifs de repère : anneau sombre et cœur plein de 3 × 3.
    let finder = |x0: usize, y0: usize| {
        for y in 0..7 {
            for x in 0..7 {
                let edge = x == 0 || x == 6 || y == 0 || y == 6;
                let core = (2..=4).contains(&x) && (2..=4).contains(&y);
                assert_eq!(dark(x0 + x, y0 + y), edge || core);
            }
        }
    };
    finder(quiet, quiet);
    finder(side - quiet - 7, quiet);
    finder(quiet, side - quiet - 7);
}

#[test]
fn grid_columns_follow_available_width() {
    assert_eq!(grid_columns(1312.0, 168.0, 14.0), 7);
    assert_eq!(grid_columns(800.0, 168.0, 14.0), 4);
    assert_eq!(grid_columns(100.0, 168.0, 14.0), 1);
    assert_eq!(grid_columns(0.0, 168.0, 14.0), 1);
}

#[test]
fn cover_uv_crops_to_the_card_without_distorting() {
    let square = cover_uv(egui::vec2(1024.0, 1024.0), 2.0 / 3.0);
    assert!((square.width() - 2.0 / 3.0).abs() < 0.001);
    assert!((square.left() - 1.0 / 6.0).abs() < 0.001);
    assert!((square.height() - 1.0).abs() < 0.001);

    let portrait = cover_uv(egui::vec2(600.0, 900.0), 2.0 / 3.0);
    assert!((portrait.width() - 1.0).abs() < 0.001);
    assert!((portrait.height() - 1.0).abs() < 0.001);

    let wide = cover_uv(egui::vec2(1920.0, 1080.0), 2.0 / 3.0);
    assert!((wide.width() - 0.375).abs() < 0.001);
    assert!((wide.left() - 0.3125).abs() < 0.001);
    assert!((wide.height() - 1.0).abs() < 0.001);
}

#[test]
fn system_emoji_fonts_come_before_the_bundled_ones() {
    // Les émojis de la machine passent devant Noto Emoji du moteur, sans
    // dépasser les polices de texte ni les autres écritures.
    let chain = fallback_chain(
        vec![
            "Ubuntu-Light".to_owned(),
            "NotoEmoji-Regular".to_owned(),
            "emoji-icon-font".to_owned(),
        ],
        vec!["system-0".to_owned()],
        vec!["system-1".to_owned(), "system-2".to_owned()],
    );
    assert_eq!(
        chain,
        vec![
            "Ubuntu-Light",
            "system-0",
            "NotoEmoji-Regular",
            "emoji-icon-font",
            "system-1",
            "system-2",
        ]
    );
    // Sans émoji système, rien ne bouge ; sans émoji embarqué, ils ferment
    // la marche sans écraser les écritures.
    assert_eq!(
        fallback_chain(
            vec!["Latin".to_owned(), "Emoji".to_owned()],
            Vec::new(),
            Vec::new()
        ),
        vec!["Latin", "Emoji"]
    );
    assert_eq!(
        fallback_chain(
            vec!["Latin".to_owned()],
            vec!["system-0".to_owned()],
            vec!["system-1".to_owned()]
        ),
        vec!["Latin", "system-0", "system-1"]
    );
}

#[test]
fn covers_reject_landscape_images() {
    // Le hero 1920×620 d'un jeu sans portrait ne doit jamais devenir une
    // pochette : recadré en 2:3, il ne montre qu'une bande de ciel.
    assert!(!portrait_cover(1920, 620));
    assert!(!portrait_cover(460, 215));
    assert!(portrait_cover(300, 450));
    assert!(portrait_cover(1024, 1024));
    assert!(!portrait_cover(0, 0));
}

#[test]
fn topbar_sections_can_be_reordered_by_dragging() {
    let ctx = egui::Context::default();
    install_fonts(&ctx);
    apply_theme(&ctx);
    let mut pages = vec![Page::Library, Page::Boutiques];
    let mut hidden = Vec::new();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(480.0, 320.0));
    let origin = std::cell::Cell::new(egui::Pos2::ZERO);
    // Les pastilles sont peintes à leur place réelle : le test lit leur
    // rectangle plutôt que de supposer une largeur, qui dépend de la langue.
    let mut frame = |ctx: &egui::Context,
                     events: Vec<egui::Event>,
                     pages: &mut Vec<Page>|
     -> Vec<(String, egui::Rect)> {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    origin.set(ui.cursor().min);
                    topbar_order(ui, pages, &mut hidden);
                });
            },
        );
        fn collect(shape: &egui::Shape, texts: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => texts.push((
                    text.galley.text().to_string(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, texts);
                    }
                }
                _ => {}
            }
        }
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut texts);
        }
        texts
    };
    let press = |pos: egui::Pos2| {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
        ]
    };
    let release = |pos: egui::Pos2| {
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }]
    };
    fn label_center(texts: &[(String, egui::Rect)], page: Page) -> egui::Pos2 {
        let label = page_label(page);
        texts
            .iter()
            .find(|(text, _)| text == label)
            .map(|(_, rect)| rect.center())
            .unwrap_or_else(|| {
                let painted: Vec<&str> = texts.iter().map(|(text, _)| text.as_str()).collect();
                panic!("onglet « {label} » absent : {painted:?}")
            })
    }
    let texts = frame(&ctx, Vec::new(), &mut pages);
    let library = label_center(&texts, Page::Library);
    let stores = label_center(&texts, Page::Boutiques);
    let _ = origin.get();
    // Le petit déplacement initial amorce le glissement sur la pastille.
    frame(&ctx, press(library), &mut pages);
    frame(
        &ctx,
        vec![egui::Event::PointerMoved(egui::pos2(library.x + 20.0, library.y))],
        &mut pages,
    );
    frame(&ctx, vec![egui::Event::PointerMoved(stores)], &mut pages);
    frame(&ctx, release(stores), &mut pages);
    assert_eq!(pages, vec![Page::Boutiques, Page::Library]);

    // Un second glissement remet la bibliothèque en tête, une fois sa
    // nouvelle place connue.
    let texts = frame(&ctx, Vec::new(), &mut pages);
    let library = label_center(&texts, Page::Library);
    let stores = label_center(&texts, Page::Boutiques);
    frame(&ctx, press(library), &mut pages);
    frame(
        &ctx,
        vec![egui::Event::PointerMoved(egui::pos2(library.x + 20.0, library.y))],
        &mut pages,
    );
    frame(&ctx, vec![egui::Event::PointerMoved(stores)], &mut pages);
    frame(&ctx, release(stores), &mut pages);
    assert_eq!(pages, vec![Page::Library, Page::Boutiques]);
}

#[test]
fn topbar_keeps_the_saved_order_and_appends_missing_sections() {
    let order: Vec<String> = ["boutiques", "library"]
        .iter()
        .map(|key| (*key).to_string())
        .collect();
    let pages = topbar_pages(&order);
    assert_eq!(&pages[..2], &[Page::Boutiques, Page::Library][..]);
    assert_eq!(pages.len(), main_pages().len());
    assert_eq!(
        pages[2..].to_vec(),
        vec![
            Page::Home,
            Page::Collections,
            Page::News,
            Page::Downloads,
            Page::Controllers,
            Page::Profile,
            Page::Captures
        ]
    );

    // Une clé inconnue et un doublon sont ignorés, sans perdre de section.
    let messy: Vec<String> = ["downloads", "inconnue", "downloads"]
        .iter()
        .map(|key| (*key).to_string())
        .collect();
    let pages = topbar_pages(&messy);
    assert_eq!(pages.len(), main_pages().len());
    assert_eq!(pages[0], Page::Downloads);
    assert_eq!(topbar_pages(&[]).len(), main_pages().len());
}

#[test]
fn startup_and_hidden_sections_preserve_defaults_and_fallbacks() {
    let mut general = lapinux_core::config::GeneralConfig::default();
    assert_eq!(
        startup_page(&general, theme::ThemeLayout::Classic),
        Page::Home
    );
    assert_eq!(
        startup_page(&general, theme::ThemeLayout::Nx),
        Page::Library
    );
    general.startup_page = "library".into();
    general.topbar_hidden = vec!["home".into()];
    assert_eq!(
        startup_page(&general, theme::ThemeLayout::Classic),
        Page::Library
    );
    assert!(
        !visible_topbar_pages(&general.topbar_order, &general.topbar_hidden).contains(&Page::Home)
    );
    general.topbar_hidden.push("library".into());
    assert_eq!(
        startup_page(&general, theme::ThemeLayout::Classic),
        Page::Collections
    );
    general.topbar_hidden = main_pages()
        .iter()
        .map(|(page, _)| page.key().to_string())
        .collect();
    assert_eq!(
        visible_topbar_pages(&[], &general.topbar_hidden),
        vec![Page::Home]
    );
}

#[test]
fn navigation_falls_back_when_the_start_or_current_page_is_hidden() {
    let mut general = lapinux_core::config::GeneralConfig::default();
    general.startup_page = "library".into();
    general.topbar_hidden = vec!["home".into()];
    assert_eq!(
        startup_page(&general, theme::ThemeLayout::Classic),
        Page::Library
    );
    assert_eq!(page_after_navigation(Page::Home, &general), Page::Library);
    assert_eq!(
        page_after_navigation(Page::Library, &general),
        Page::Library
    );

    // Toutes les sections masquées : la première reste visible et la page
    // courante ne peut pas rester sur un écran sans onglet.
    general.topbar_hidden = main_pages()
        .iter()
        .map(|(page, _)| page.key().to_string())
        .collect();
    let repaired = {
        let mut config = lapinux_core::Config::default();
        config.general = general;
        config.normalized()
    };
    assert!(!repaired.general.topbar_hidden.contains(&"home".to_string()));
    assert_eq!(
        page_after_navigation(Page::Profile, &repaired.general),
        Page::Home
    );
}

#[test]
fn the_navigation_settings_and_topbar_paint_the_hidden_choice() {
    let ctx = egui::Context::default();
    install_fonts(&ctx);
    apply_theme(&ctx);
    let mut config = lapinux_core::Config::default().normalized();
    config.general.startup_page = "library".into();
    config.general.topbar_hidden = vec!["home".into()];
    let mut output = None;
    for _ in 0..2 {
        let full = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(980.0, 1200.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        navigation_settings_card(ui, &mut config);
                    });
                });
            },
        );
        output = Some(full);
    }
    fn painted(output: &egui::FullOutput) -> Vec<String> {
        fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }
    let texts = painted(&output.unwrap());
    assert!(texts
        .iter()
        .any(|text| text == &lapinux_core::i18n::t("settings.appearance.hidden_sections")));
    assert!(texts
        .iter()
        .any(|text| text == &lapinux_core::i18n::t("settings.appearance.startup_page")));
    assert!(texts
        .iter()
        .any(|text| text == &lapinux_core::i18n::t("settings.appearance.settings_control_note")));
    // L'Accueil masqué n'apparaît que dans la liste des sections à remettre,
    // jamais dans l'aperçu de la barre.
    assert_eq!(
        texts
            .iter()
            .filter(|text| *text == page_label(Page::Home))
            .count(),
        1,
        "{texts:?}"
    );

    let pages = visible_topbar_pages(&config.general.topbar_order, &config.general.topbar_hidden);
    assert!(!pages.contains(&Page::Home));
    let mut tab_texts = Vec::new();
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1050.0, 120.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                main_tabs(ui, Page::Library, None, &pages);
            });
        },
    );
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1050.0, 120.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                main_tabs(ui, Page::Library, None, &pages);
            });
        },
    );
    tab_texts.extend(painted(&output));
    assert!(!tab_texts.iter().any(|text| text == page_label(Page::Home)));
    assert!(tab_texts
        .iter()
        .any(|text| text == page_label(Page::Library)));
}

#[test]
fn clicking_an_icon_opens_its_page() {
    let ctx = egui::Context::default();
    install_fonts(&ctx);
    apply_theme(&ctx);
    // On part de Téléchargements : le premier onglet est donc réduit à
    // son icône, à une position stable quel que soit le libellé ouvert.
    let mut page = Page::Downloads;
    let origin = std::cell::Cell::new(egui::Pos2::ZERO);
    let frame = |ctx: &egui::Context, events: Vec<egui::Event>, page: &mut Page| {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 120.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    origin.set(ui.cursor().min);
                    if let Some(next) = main_tabs(ui, *page, None, &topbar_pages(&[])) {
                        *page = next;
                    }
                });
            },
        );
    };
    frame(&ctx, Vec::new(), &mut page);
    // Centre de l'icône du premier onglet : 2 px de marge + 12 px de
    // padding + 9 px (demi-icône).
    let target = egui::pos2(origin.get().x + 23.0, origin.get().y + 17.0);
    let press = vec![
        egui::Event::PointerMoved(target),
        egui::Event::PointerButton {
            pos: target,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        },
    ];
    frame(&ctx, press, &mut page);
    let release = vec![egui::Event::PointerButton {
        pos: target,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    }];
    frame(&ctx, release, &mut page);
    assert_eq!(page, Page::Home);
}

/// Chaque segment de la barre répond à sa commande, dans l'ordre affiché.
#[test]
fn the_friends_icon_sits_between_fullscreen_and_settings() {
    let ctx = egui::Context::default();
    install_fonts(&ctx);
    apply_theme(&ctx);
    let origin = std::cell::Cell::new(egui::Pos2::ZERO);
    let clicks = std::cell::Cell::new((false, false, false, false));
    let frame = |ctx: &egui::Context, events: Vec<egui::Event>| {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(320.0, 120.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    origin.set(ui.cursor().min);
                    clicks.set(topbar_controls(ui, false, false, false, false, false, None));
                });
            },
        );
    };
    let click = |x: f32, y: f32| {
        let target = egui::pos2(x, y);
        let press = vec![
            egui::Event::PointerMoved(target),
            egui::Event::PointerButton {
                pos: target,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
        ];
        let release = vec![egui::Event::PointerButton {
            pos: target,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }];
        (press, release)
    };
    frame(&ctx, Vec::new());
    let origin = origin.get();
    let y = origin.y + 17.0;
    // Le groupe fait quatre segments : plein écran, notifications, amis, réglages.
    let (press, release) = click(origin.x + 17.0, y);
    frame(&ctx, press);
    frame(&ctx, release);
    assert_eq!(
        clicks.get(),
        (false, true, false, false),
        "le plein écran reste à gauche"
    );
    let (press, release) = click(origin.x + 51.0, y);
    frame(&ctx, press);
    frame(&ctx, release);
    assert_eq!(
        clicks.get(),
        (false, false, false, true),
        "les notifications suivent le plein écran"
    );
    let (press, release) = click(origin.x + 85.0, y);
    frame(&ctx, press);
    frame(&ctx, release);
    assert_eq!(
        clicks.get(),
        (false, false, true, false),
        "Amis garde son bouton"
    );
    let (press, release) = click(origin.x + 119.0, y);
    frame(&ctx, press);
    frame(&ctx, release);
    assert_eq!(
        clicks.get(),
        (true, false, false, false),
        "les réglages gardent la droite"
    );
}

#[test]
fn steamgriddb_assets_prefer_png_and_the_right_shape() {
    let logos = serde_json::json!({
        "data": [
            {"url": "big-jpg", "mime": "image/jpeg", "width": 3840, "height": 1240},
            {"url": "png", "mime": "image/png", "width": 1600, "height": 600},
            {"url": "nsfw", "mime": "image/png", "width": 4000, "height": 2000, "nsfw": true}
        ]
    });
    assert_eq!(
        sgdb_asset_pick(&logos, SteamArt::Logo).as_deref(),
        Some("png")
    );

    let icons = serde_json::json!({
        "data": [
            {"url": "wide", "mime": "image/png", "width": 1024, "height": 512},
            {"url": "square", "mime": "image/png", "width": 512, "height": 512}
        ]
    });
    assert_eq!(
        sgdb_asset_pick(&icons, SteamArt::Icon).as_deref(),
        Some("square")
    );

    let covers = serde_json::json!({
        "data": [
            {"url": "wide", "mime": "image/png", "width": 920, "height": 430},
            {"url": "portrait", "mime": "image/png", "width": 600, "height": 900},
            {"url": "square", "mime": "image/png", "width": 1024, "height": 1024}
        ]
    });
    assert_eq!(
        sgdb_asset_pick(&covers, SteamArt::Cover).as_deref(),
        Some("portrait")
    );
}

#[test]
fn community_gallery_slots_are_marked_steamgriddb() {
    assert_eq!(ArtSlotSource::Community.label(), "SGDB");
    assert!(ArtSlotSource::Community
        .tooltip()
        .is_some_and(|tooltip| tooltip.contains("SteamGridDB")));
    assert_eq!(ArtSlotSource::Published.label(), "");
    assert_eq!(ArtSlotSource::StoreFallback.label(), "Repli");
}

#[test]
fn partial_rows_are_centered() {
    assert_eq!(row_padding(1000.0, 2, 168.0, 14.0), 325.0);
    assert_eq!(row_padding(300.0, 2, 168.0, 14.0), 0.0);
    assert_eq!(row_padding(1000.0, 0, 168.0, 14.0), 0.0);
}

#[test]
fn logo_asset_is_embedded_and_transparent() {
    let image = image::load_from_memory(include_bytes!("../../../assets/lapinux-logo.png"))
        .unwrap()
        .to_rgba8();
    assert!(image.width() > 0 && image.height() > 0);
    assert_eq!(image.get_pixel(0, 0)[3], 0);
}

#[test]
fn dualsense_features_include_microphone_and_adaptive_triggers() {
    let device = controller(
        "DualSense Wireless Controller",
        Some("02:00:00:00:00:01"),
        true,
    );
    let identity = pad_identity(&device);
    let features = controller_features(&device, &identity, None);
    for expected in [
        "bluetooth",
        "touchpad",
        "gyro",
        "accel",
        "haptics",
        "adaptive_triggers",
        "led",
        "microphone",
        "speaker",
        "headset_jack",
    ] {
        assert!(
            features.contains(&expected),
            "fonctionnalité manquante : {expected}"
        );
    }
}

#[test]
fn steam_puck_keeps_its_key_when_its_event_number_changes() {
    let mut first = controller("Wireless Steam Controller", None, true);
    first.vendor = 0x28de;
    first.product = 0x1304;
    first.path = PathBuf::from("/dev/input/event27");
    first.udev.serial = Some("Valve_Software_Steam_Controller_Puck_FXB9954802F92".into());
    let mut second = first.clone();
    second.path = PathBuf::from("/dev/input/event28");

    assert_eq!(controller_key(&first), controller_key(&second));
    assert!(controller_key(&first).contains("fxb9954802f92"));
}

#[test]
fn a_pad_never_inherits_the_mask_left_on_its_evdev_node() {
    // Le noyau réattribue les numéros de nœud : le masquage enregistré
    // pour un dongle 2,4 GHz sur `event31` ne doit pas s'appliquer à la
    // DualSense qui occupe ce nœud aujourd'hui — sinon sa fiche et son
    // test en direct perdent sticks, gâchettes et vibrations.
    let mut dongle = controller(lapinux_core::i18n::t("pad.radio_2_4ghz"), None, true);
    dongle.connection = lapinux_core::model::DeviceLink::Dongle;
    dongle.path = PathBuf::from("/dev/input/event31");
    let mut mask = HashMap::new();
    mask.insert(controller_key(&dongle), true);

    let mut dualsense = controller(
        "DualSense Wireless Controller",
        Some("02:00:00:00:00:01"),
        true,
    );
    dualsense.path = PathBuf::from("/dev/input/event31");
    dualsense.force_feedback = true;
    dualsense.axes = [
        "ABS_X",
        "ABS_Y",
        "ABS_Z",
        "ABS_RX",
        "ABS_RY",
        "ABS_RZ",
        "ABS_HAT0X",
        "ABS_HAT0Y",
    ]
    .iter()
    .map(|axis| (*axis).to_string())
    .collect();

    assert_eq!(
        pad_setting(&mask, &dualsense),
        None,
        "un masquage laissé sur un ancien chemin ne concerne pas cette manette"
    );
    let identity = identify(dualsense.vendor, dualsense.product, &dualsense.name);
    let features = controller_features(&dualsense, &identity, None);
    for expected in ["sticks", "triggers", "haptics"] {
        assert!(
            features.contains(&expected),
            "la DualSense doit annoncer {expected}"
        );
    }
    // La manette qui a écrit ce masquage le garde : c'est le seul nœud
    // dont le chemin l'identifie.
    assert_eq!(pad_setting(&mask, &dongle), Some(&true));
}

#[test]
fn bluetooth_stays_listed_even_on_a_wired_link() {
    let wireless = controller(
        "DualSense Wireless Controller",
        Some("02:00:00:00:00:01"),
        true,
    );
    let identity = identify(wireless.vendor, wireless.product, &wireless.name);
    assert!(controller_features(&wireless, &identity, None).contains(&"bluetooth"));

    // Branchée, la manette garde Bluetooth dans ses capacités : la liste
    // décrit la manette, pas la liaison du moment.
    let wired = controller("DualSense Wireless Controller", None, true);
    let identity = identify(wired.vendor, wired.product, &wired.name);
    assert!(controller_features(&wired, &identity, None).contains(&"bluetooth"));
}

#[test]
fn dualshock_4_features_include_the_light_bar() {
    let mut device = controller("Wireless Controller", None, true);
    device.product = 0x09cc;
    let identity = pad_identity(&device);
    let features = controller_features(&device, &identity, None);
    assert!(
        features.contains(&"led"),
        "la barre lumineuse de la DualShock 4 doit être listée"
    );
}

#[test]
fn hue_selector_maps_the_primary_colors() {
    assert_eq!(hue_rgb(0.0), [255, 0, 0]);
    assert_eq!(hue_rgb(120.0), [0, 255, 0]);
    assert_eq!(hue_rgb(240.0), [0, 0, 255]);
    assert_eq!(hue_rgb(360.0), [255, 0, 0]);
    assert_eq!(hue_name(0.0), "Rouge");
    assert_eq!(hue_name(210.0), "Bleu");
    assert_eq!(hue_name(120.0), "Vert");
}

#[test]
fn dpad_accepts_buttons_and_hat_axes() {
    let mut pad = lapinux_input::PadState::default();
    pad.pressed.push("Haut".into());
    pad.pressed.push("Gauche".into());
    assert_eq!(dpad_axes(&pad), (-1.0, -1.0));

    // Nouvelle écriture neutre, à côté des libellés historiques.
    let mut pad = lapinux_input::PadState::default();
    pad.pressed.push("Left".into());
    assert_eq!(dpad_axes(&pad), (-1.0, 0.0));
    let mut pad = lapinux_input::PadState::default();
    pad.pressed.push("Right".into());
    assert_eq!(dpad_axes(&pad), (1.0, 0.0));

    let mut pad = lapinux_input::PadState::default();
    pad.axes.insert("dpad_x".into(), 1.0);
    pad.axes.insert("dpad_y".into(), -1.0);
    assert_eq!(dpad_axes(&pad), (1.0, -1.0));
}

#[test]
fn exotic_pad_lists_battery_paddles_and_extra_buttons() {
    let mut device = controller("Zebra Pad", Some("AA:BB:CC:DD:EE:FF"), true);
    device.vendor = 0x1234;
    device.product = 0x5678;
    device.battery = Some(64);
    device.battery_status = Some("Discharging".into());
    device.force_feedback = true;
    device.touchpad = true;
    device.button_names = vec![
        "A".into(),
        "B".into(),
        "X".into(),
        "Y".into(),
        "LB".into(),
        "RB".into(),
        "LT".into(),
        "RT".into(),
        "Palette L".into(),
        "Palette R".into(),
        "Bouton 0x2e0".into(),
    ];
    let identity = pad_identity(&device);
    assert_eq!(identity.brand, Some(Brand::Generic));
    let features = controller_features(&device, &identity, None);
    for expected in [
        "battery",
        "vibrations",
        "touchpad",
        "paddles",
        "extra_buttons",
    ] {
        assert!(
            features.contains(&expected),
            "fonctionnalité manquante : {expected}"
        );
    }
}

#[test]
fn bluez_battery_survives_usb_and_bluetooth_merge() {
    let mut wired = controller(
        "DualSense Wireless Controller",
        Some("02:00:00:00:00:01"),
        true,
    );
    wired.connection = lapinux_core::model::DeviceLink::Usb;
    wired.battery = None;
    let mut bluez = controller("DS5", Some("02:00:00:00:00:01"), false);
    bluez.battery = Some(82);
    bluez.battery_status = Some("Discharging".into());
    let merged = merge_controllers(vec![wired], vec![bluez]);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].battery, Some(82));
    assert_eq!(merged[0].battery_status.as_deref(), Some("Discharging"));
    assert!(!merged[0].button_names.is_empty());
}

#[test]
fn xbox_360_profile_does_not_invent_missing_hardware() {
    // 045e:028e est aussi l'identifiant de receveurs et de clones
    // génériques : la famille seule ne doit pas promettre gâchettes,
    // croix ou vibrations qu'evdev n'expose pas.
    let mut device = controller("Microsoft X-Box 360 pad", None, true);
    device.vendor = 0x045e;
    device.product = 0x028e;
    let identity = pad_identity(&device);
    assert_eq!(identity.family, Some("Xbox 360"));
    assert_eq!(
        controller_features(&device, &identity, None),
        vec!["sticks"]
    );
    assert!(!has_dpad(&device));
    assert!(!has_triggers(&device));
    assert!(!has_shoulders(&device));
}

#[test]
fn digital_triggers_are_detected_without_axes() {
    let mut device = controller("Generic USB Gamepad", None, true);
    device.axes = vec!["ABS_X".into(), "ABS_Y".into()];
    device.button_names = vec!["A".into(), "B".into(), "LT".into(), "RT".into()];
    assert!(has_triggers(&device));
    assert!(!has_shoulders(&device));
}

#[test]
fn a_bluetooth_pad_never_inherits_a_setting_left_on_its_event_node() {
    // Le noyau réutilise les numéros de nœud : la DualSense se retrouve
    // sur l'`event` du dongle 2,4 GHz. Le masquage laissé par ce dernier
    // ne doit plus la concerner, sinon ses sticks, ses gâchettes et ses
    // vibrations disparaissent de la fiche et du test en direct.
    let pad = controller(
        "DualSense Wireless Controller",
        Some("02:00:00:00:00:01"),
        true,
    );
    let path_key = format!("path:{}", pad.path.display());
    let mut hide: HashMap<String, bool> = HashMap::new();
    hide.insert(path_key.clone(), true);
    assert_eq!(
        pad_setting(&hide, &pad),
        None,
        "une manette identifiée par son adresse ne lit pas les réglages de chemin"
    );
    let mut names: HashMap<String, String> = HashMap::new();
    names.insert(path_key.clone(), "SNES Aliexpress".into());
    let identity = identify(pad.vendor, pad.product, &pad.name);
    assert_ne!(
        pad_shown_name(&names, &pad, &identity),
        "SNES Aliexpress",
        "le surnom d'une autre manette ne doit pas être reprise"
    );

    // Une manette sans adresse (dongle, USB) reste servie par son ancienne
    // clé de chemin, et le nouveau couple vendeur:produit passe devant.
    let mut dongle = controller(lapinux_core::i18n::t("pad.radio_2_4ghz"), None, true);
    dongle.path = pad.path.clone();
    assert_eq!(
        pad_setting(&hide, &dongle),
        Some(&true),
        "la manette du dongle garde son réglage de chemin"
    );
}

#[test]
fn unknown_pad_only_lists_detected_inputs() {
    let mut device = controller("Generic USB Gamepad", None, true);
    device.vendor = 0x1234;
    device.product = 0x5678;
    device.axes = vec![
        "ABS_X".into(),
        "ABS_Y".into(),
        "ABS_HAT0X".into(),
        "ABS_HAT0Y".into(),
    ];
    let identity = pad_identity(&device);
    assert_eq!(
        controller_features(&device, &identity, None),
        vec!["sticks", "dpad"]
    );
}

#[test]
fn digital_triggers_are_detected() {
    let mut pad = lapinux_input::PadState::default();
    assert!(!analog_triggers(&pad));
    pad.axes.insert("left_trigger".into(), 0.0);
    assert!(analog_triggers(&pad));
}

#[test]
fn face_buttons_follow_the_brand() {
    assert_eq!(brand_button_text("A", Some(Brand::PlayStation)), "Croix");
    assert_eq!(brand_button_text("B", Some(Brand::PlayStation)), "Rond");
    assert_eq!(brand_button_text("X", Some(Brand::PlayStation)), "Triangle");
    assert_eq!(brand_button_text("Y", Some(Brand::PlayStation)), "Carré");
    assert_eq!(brand_button_text("A", Some(Brand::Nintendo)), "B");
    assert_eq!(brand_button_text("B", Some(Brand::Nintendo)), "A");
    assert_eq!(brand_button_text("LT", Some(Brand::PlayStation)), "L2");
    assert_eq!(brand_button_text("RT", Some(Brand::Nintendo)), "ZR");
    assert_eq!(
        brand_button_text("Start", Some(Brand::PlayStation)),
        lapinux_core::i18n::t("settings.general.options")
    );
    assert_eq!(brand_button_text("LB", Some(Brand::Xbox)), "LB");
    assert_eq!(brand_button_text("Select", Some(Brand::Xbox)), "View");
    assert_eq!(brand_button_text("Start", Some(Brand::Xbox)), "Menu");
    assert_eq!(brand_button_text("Select", Some(Brand::Generic)), "View");
    assert_eq!(brand_button_text("Start", Some(Brand::Generic)), "Menu");
    assert_eq!(brand_button_text("Guide", Some(Brand::Steam)), "Steam");
    assert_eq!(
        face_labels(FaceButtonProfile::Nintendo),
        ["X", "A", "B", "Y"]
    );
    assert_eq!(face_labels(FaceButtonProfile::Xbox), ["Y", "B", "A", "X"]);
}

#[test]
fn navigation_and_brand_glyphs_are_distinct() {
    let glyphs = [
        nav_glyph(Page::Library),
        nav_glyph(Page::Downloads),
        nav_glyph(Page::Controllers),
        brand_glyph(Brand::PlayStation),
        brand_glyph(Brand::Xbox),
        brand_glyph(Brand::Nintendo),
        brand_glyph(Brand::Steam),
    ];
    for (index, glyph) in glyphs.iter().enumerate() {
        assert!(!glyph.is_control(), "glyphe {index} invalide");
        assert!(
            !glyphs[..index].contains(glyph),
            "le glyphe {glyph:?} est utilisé deux fois"
        );
    }
    // Le sous-ensemble doit rester embarqué : une police vide passerait
    // les tests sans qu'aucune icône ne s'affiche.
    assert!(
        include_bytes!("../../../assets/fonts/RemixIcons.ttf").len() > 1000,
        "la police d'icônes semble vide"
    );
}

#[test]
fn platform_icons_fit_inside_the_version_button() {
    /// Union des formes peintes : l'emblème doit tenir dans la moitié du
    /// bouton, sans déborder sur le bord arrondi.
    fn walk(shape: &egui::Shape, bounds: &mut egui::Rect) {
        let painted = match shape {
            egui::Shape::Circle(circle) => Some(
                egui::Rect::from_center_size(
                    circle.center,
                    egui::vec2(circle.radius * 2.0, circle.radius * 2.0),
                )
                .expand(circle.stroke.width),
            ),
            egui::Shape::Path(path) => {
                Some(egui::Rect::from_points(path.points.as_slice()).expand(path.stroke.width))
            }
            egui::Shape::LineSegment { points, stroke } => {
                Some(egui::Rect::from_points(points).expand(stroke.width))
            }
            // Le fond du panneau est un grand rectangle plein : seuls les
            // carreaux de l'emblème Windows sont de cette taille.
            egui::Shape::Rect(rect) if rect.rect.width() <= 20.0 => Some(rect.rect),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, bounds);
                }
                None
            }
            _ => None,
        };
        if let Some(rect) = painted {
            *bounds = bounds.union(rect);
        }
    }

    for platform in ["linux", "windows", "macos"] {
        let ctx = egui::Context::default();
        apply_theme(&ctx);
        let center = egui::pos2(60.0, 40.0);
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                paint_platform_icon(ui.painter(), center, platform, TEXT(), 0.85);
            });
        });
        let mut bounds = egui::Rect::NOTHING;
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut bounds);
        }
        assert!(bounds.is_positive(), "{platform} : rien de peint");
        assert!(
            bounds.width() <= 14.0 && bounds.height() <= 14.0,
            "{platform} déborde : {}×{}",
            bounds.width(),
            bounds.height()
        );
        assert!(
            (bounds.center().x - center.x).abs() <= 2.0,
            "{platform} décalé : {}",
            bounds.center().x - center.x
        );
    }
}

#[test]
fn steamgriddb_search_terms_are_url_encoded() {
    assert_eq!(url_encode("Portal 2"), "Portal%202");
    assert_eq!(url_encode("L'Ombre d'Œdipe"), "L%27Ombre%20d%27%C5%92dipe");
}

#[test]
fn steamgriddb_accepte_objet_ou_tableau() {
    let object = serde_json::json!({ "data": { "id": 42 } });
    let array = serde_json::json!({ "data": [{ "id": 7 }, { "id": 8 }] });
    let empty = serde_json::json!({ "data": [] });
    assert_eq!(first_game_id(&object), Some(42));
    assert_eq!(first_game_id(&array), Some(7));
    assert_eq!(first_game_id(&empty), None);
}

#[test]
fn library_sources_can_be_deselected_independently() {
    let owned = LibraryEntry {
        playtime: 0,
        acquired: None,
        updated: 0,
        last_played: 0,
        appid: 1,
        name: "Owned".into(),
        installed: true,
        on_disk: true,
        steam: true,
        family: false,
        family_available: false,
        update: None,
        store: None,
        cover: String::new(),
        platforms: Vec::new(),
        mobile: false,
        size: None,
        publishers: Vec::new(),
        developers: Vec::new(),
        genres: Vec::new(),
    };
    let family = LibraryEntry {
        playtime: 0,
        acquired: None,
        updated: 0,
        last_played: 0,
        appid: 2,
        name: "Family".into(),
        installed: false,
        on_disk: false,
        steam: true,
        family: true,
        family_available: false,
        update: None,
        store: None,
        cover: String::new(),
        platforms: Vec::new(),
        mobile: false,
        size: None,
        publishers: Vec::new(),
        developers: Vec::new(),
        genres: Vec::new(),
    };
    let disk_only = LibraryEntry {
        playtime: 0,
        acquired: None,
        updated: 0,
        last_played: 0,
        appid: 3,
        name: "Disk only".into(),
        installed: true,
        on_disk: true,
        steam: false,
        family: false,
        family_available: false,
        update: None,
        store: None,
        cover: String::new(),
        platforms: Vec::new(),
        mobile: false,
        size: None,
        publishers: Vec::new(),
        developers: Vec::new(),
        genres: Vec::new(),
    };
    // Un jeu de boutique n'appartient à aucune source Steam : il suit sa
    // propre pastille.
    let epic = LibraryEntry {
        playtime: 0,
        acquired: None,
        updated: 0,
        last_played: 0,
        appid: 4,
        name: "Epic".into(),
        installed: false,
        on_disk: false,
        steam: false,
        family: false,
        family_available: false,
        update: None,
        store: Some(lapinux_core::stores::Store::Epic),
        cover: "https://cdn/epic.jpg".into(),
        platforms: vec!["windows".into()],
        mobile: false,
        size: Some(3_000_000_000),
        publishers: Vec::new(),
        developers: Vec::new(),
        genres: Vec::new(),
    };
    let mut epic_mobile = epic.clone();
    epic_mobile.appid = 5;
    epic_mobile.name = "Epic mobile".into();
    epic_mobile.platforms = Vec::new();
    epic_mobile.mobile = true;
    let mut sources: SourceFilter = SourceFilter::default();
    assert!(sources.keeps(&owned));
    assert!(sources.keeps(&family));
    assert!(sources.keeps(&disk_only));

    sources.disk = false;
    assert!(sources.keeps(&owned));
    assert!(!sources.keeps(&disk_only));

    sources.steam = false;
    assert!(sources.keeps(&family));
    assert!(!sources.keeps(&owned));

    sources.family = false;
    // La source Epic reste active : la bibliothèque n'est pas vide.
    assert!(sources.any());
    assert!(!sources.keeps(&family));
    assert!(sources.keeps(&epic));
    assert!(sources.keeps(&epic_mobile));

    // Les deux pastilles Epic sont indépendantes, comme le compte et la
    // famille Steam : chacune peut être retirée sans toucher à l'autre.
    sources.epic_pc = false;
    sources.epic_mobile = true;
    assert!(!sources.keeps(&epic));
    assert!(sources.keeps(&epic_mobile));
    sources.epic_pc = true;
    sources.epic_mobile = false;
    assert!(sources.keeps(&epic));
    assert!(!sources.keeps(&epic_mobile));
    sources.epic_pc = true;
    sources.epic_mobile = true;
    assert!(sources.keeps(&epic));
    assert!(sources.keeps(&epic_mobile));

    sources.epic_pc = false;
    sources.epic_mobile = false;
    for on in sources.stores.iter_mut() {
        *on = false;
    }
    assert!(!sources.any());
    assert!(!sources.keeps(&epic));
}

#[test]
fn activity_trash_targets_the_right_history() {
    let session = ActivityDelete::Session {
        appid: 42,
        started: 7,
    };
    assert_eq!(session.tooltip(), "Supprimer cette partie");
    assert!(matches!(
        session.request(),
        ActivityRequest::DeleteSession {
            appid: 42,
            started: 7
        }
    ));

    let download = ActivityDelete::Job {
        id: "game-42-1".into(),
    };
    assert_eq!(download.tooltip(), "Supprimer ce téléchargement");
    assert!(matches!(
        download.request(),
        ActivityRequest::DeleteJob { id } if id == "game-42-1"
    ));
}

#[test]
fn log_lines_are_split_into_date_time_level_and_message() {
    let (date, time, level, message) =
        log_fields("2026-11-04T09:43:52.002460Z  INFO lapinux_core: scan").unwrap();
    assert_eq!(date, "04/11");
    assert_eq!(time, "09:43:52");
    assert_eq!(level, "INFO");
    assert_eq!(message, "lapinux_core: scan");
    // Une sortie brute de jeu et un en-tête de session ne sont pas
    // déguisés en lignes de tracing.
    assert!(log_fields("jeu : swapchain créée").is_none());
    assert!(log_fields("=== session 440 · 04/11/2026 à 09:43 ===").is_none());
}

#[test]
fn demos_follow_the_library_switch() {
    let demo = SteamCatalogGame {
        appid: 1,
        name: "Demo".into(),
        installed: false,
        family: false,
        details: SteamDetails {
            app_type: "demo".into(),
            ..Default::default()
        },
        dlc: Vec::new(),
        acquired: None,
        minutes: None,
        undated_reason: None,
    };
    assert!(steam_entry_visible(&demo, true));
    assert!(!steam_entry_visible(&demo, false));
    let mut game = demo.clone();
    game.details.app_type = "game".into();
    assert!(steam_entry_visible(&game, false));
    assert!(lapinux_core::config::LibraryConfig::default().show_demos);
}

#[test]
fn a_store_identifier_change_is_not_a_new_game() {
    fn entry(appid: u32, name: &str) -> LibraryEntry {
        LibraryEntry {
            playtime: 0,
            acquired: None,
            appid,
            name: name.into(),
            installed: false,
            on_disk: false,
            steam: false,
            family: false,
            family_available: false,
            update: None,
            store: Some(lapinux_core::stores::Store::Epic),
            cover: String::new(),
            platforms: Vec::new(),
            mobile: false,
            size: None,
            publishers: Vec::new(),
            developers: Vec::new(),
            genres: Vec::new(),
            updated: 0,
            last_played: 0,
        }
    }
    // Epic a changé l'appName de Botany Manor : le nouvel appid ne doit
    // pas faire passer le même titre pour un nouveau jeu.
    let previous = SeenLibrary {
        games: vec![SeenGame {
            appid: 3_337_404_891,
            name: "Botany Manor".into(),
        }],
        checked: 0,
    };
    let entries = vec![
        entry(4_206_602_588, "Botany Manor"),
        entry(4_206_602_589, "A Short Hike"),
    ];
    let added = new_library_entries(&entries, Some(&previous));
    assert_eq!(added.len(), 1, "seul le vrai nouveau jeu reste");
    assert_eq!(added[0].name, "A Short Hike");
    assert!(new_library_entries(&entries, None).is_empty());
}

#[test]
fn steam_tools_are_hidden_without_hiding_games() {
    assert!(is_steam_tool("Proton 10.0"));
    assert!(is_steam_tool("Proton Experimental"));
    assert!(is_steam_tool("Proton 9.0 (Beta)"));
    assert!(is_steam_tool("Steam Linux Runtime 4.0"));
    assert!(is_steam_tool("Steamworks Common Redistributables"));
    assert!(!is_steam_tool("Proton Bus Simulator"));
    assert!(!is_steam_tool("Tools Up!"));
    assert!(!is_steam_tool("Arizona Sunshine"));
}

#[test]
fn catalog_filtre_installe_et_recherche() {
    let installed = SteamCatalogGame {
        acquired: None,
        minutes: None,
        undated_reason: None,
        appid: 730,
        name: "Counter-Strike 2".into(),
        installed: true,
        family: false,
        details: Default::default(),
        dlc: Vec::new(),
    };
    let missing = SteamCatalogGame {
        acquired: None,
        minutes: None,
        undated_reason: None,
        appid: 440,
        name: "Team Fortress 2".into(),
        installed: false,
        family: false,
        details: Default::default(),
        dlc: Vec::new(),
    };
    assert!(catalog_matches(&installed, CatalogFilter::All, ""));
    assert!(catalog_matches(&missing, CatalogFilter::All, ""));
    assert!(catalog_matches(&installed, CatalogFilter::Installed, ""));
    assert!(!catalog_matches(&missing, CatalogFilter::Installed, ""));
    assert!(!catalog_matches(&installed, CatalogFilter::Missing, ""));
    assert!(catalog_matches(&missing, CatalogFilter::Missing, ""));
    assert!(catalog_matches(
        &missing,
        CatalogFilter::Missing,
        "fortress"
    ));
    assert!(!catalog_matches(&installed, CatalogFilter::All, "fortress"));
}

fn controller(name: &str, address: Option<&str>, connected: bool) -> DeviceInfo {
    DeviceInfo {
        path: if connected {
            PathBuf::from(format!("/dev/input/event-{name}"))
        } else {
            PathBuf::new()
        },
        name: name.to_string(),
        vendor: 0x054c,
        product: 0x0ce6,
        gamepad: true,
        buttons: 13,
        button_names: vec!["A".into(), "B".into(), "X".into(), "Y".into()],
        axes: vec!["ABS_X".into(), "ABS_Y".into()],
        force_feedback: false,
        touchpad: false,
        connection: if address.is_some() {
            lapinux_core::model::DeviceLink::Bluetooth
        } else {
            lapinux_core::model::DeviceLink::Usb
        },
        battery: None,
        battery_status: None,
        address: address.map(String::from),
        connected,
        steam_bridge: false,
        udev: Default::default(),
    }
}

#[test]
fn topbar_lists_every_connected_pad_once() {
    let mut keyboard = controller(
        lapinux_core::i18n::t("settings.general.keyboard"),
        None,
        true,
    );
    keyboard.gamepad = false;
    let ds5 = controller(
        "DualSense Wireless Controller",
        Some("02:00:00:00:00:01"),
        true,
    );
    let mut pro = controller("Pro Controller", Some("02:00:00:00:00:02"), true);
    pro.vendor = 0x057e;
    pro.product = 0x2009;
    let offline = controller("Xbox Wireless Controller", Some("AA:BB:CC:DD:EE:FF"), false);
    // Même manette vue en USB : une seule icône dans la barre du haut.
    let mut wired = ds5.clone();
    wired.connection = lapinux_core::model::DeviceLink::Usb;
    wired.path = PathBuf::from("/dev/input/event990");

    let all = [keyboard, ds5.clone(), offline, pro, wired];
    let listed = topbar_controller_devices(&all, &[], 1_000);
    assert_eq!(
        listed.len(),
        2,
        "une icône par manette connectée, pas par lien"
    );
    assert_eq!(controller_key(listed[0]), controller_key(&ds5));
    assert_eq!(listed[1].product, 0x2009);
}

#[test]
fn a_bridged_puck_is_named_as_a_steam_controller() {
    let mut bridged = controller("Microsoft X-Box 360 pad", None, true);
    bridged.vendor = 0x045e;
    bridged.product = 0x028e;
    bridged.connection = lapinux_core::model::DeviceLink::Virtual;
    bridged.steam_bridge = true;
    let identity = pad_identity(&bridged);
    assert_eq!(
        identity.display_name(&bridged.name),
        "Steam Controller 2026"
    );

    // Sans le pont, le même pad reste une manette Xbox 360.
    bridged.steam_bridge = false;
    let identity = pad_identity(&bridged);
    assert_eq!(identity.family, Some("Xbox 360"));
}

#[test]
fn steam_puck_entry_waits_for_a_real_gamepad() {
    use crate::controllers::steam_puck_needs_entry;

    assert!(steam_puck_needs_entry(true, false, &[]));
    assert!(!steam_puck_needs_entry(false, false, &[]));
    assert!(
        !steam_puck_needs_entry(true, true, &[]),
        "le pont actif remplace le pilote"
    );

    let mut native = controller("Steam Controller", None, true);
    native.vendor = 0x28de;
    native.product = 0x1304;
    assert!(
        !steam_puck_needs_entry(true, false, &[native.clone()]),
        "un gamepad Triton exposé retire l'entrée synthétique"
    );

    native.connected = false;
    assert!(
        !steam_puck_needs_entry(true, false, &[native]),
        "une manette Steam déjà appairée garde sa propre entrée"
    );
}

#[test]
fn paired_duplicates_merge_and_offline_is_kept() {
    let mut zebra = controller("Zebra Pad", None, true);
    zebra.vendor = 0x1234;
    zebra.product = 0x5678;
    let mut pro = controller("Pro Controller", Some("02:00:00:00:00:02"), false);
    pro.vendor = 0x057e;
    pro.product = 0x2009;
    let merged = merge_controllers(
        vec![
            zebra,
            controller(
                "DualSense Wireless Controller",
                Some("02:00:00:00:00:01"),
                true,
            ),
        ],
        vec![
            controller(
                "DualSense Wireless Controller",
                Some("02:00:00:00:00:01"),
                false,
            ),
            pro,
        ],
    );
    let names: Vec<&str> = merged.iter().map(|pad| pad.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "DualSense Wireless Controller",
            "Pro Controller",
            "Zebra Pad"
        ]
    );
    assert_eq!(merged.len(), 3);
    assert_eq!(merged.iter().filter(|pad| !pad.connected).count(), 1);
}

#[test]
fn generic_pads_on_two_receivers_stay_two_cards() {
    // Le pont du Puck expose lui aussi un 045e:028e. Il ne doit pas
    // absorber la manette 2,4 GHz endormie, simple récepteur branché.
    let mut awake = controller("Microsoft X-Box 360 pad", None, true);
    awake.connection = lapinux_core::model::DeviceLink::Usb;
    awake.path = PathBuf::from("/dev/input/event262");
    let mut asleep = controller(lapinux_core::i18n::t("pad.radio_2_4ghz"), None, false);
    asleep.connection = lapinux_core::model::DeviceLink::Dongle;
    asleep.path = PathBuf::from("/dev/input/event31");
    let merged = merge_controllers(vec![awake, asleep], Vec::new());
    assert_eq!(merged.len(), 2, "deux récepteurs distincts, deux cartes");
    assert_eq!(merged.iter().filter(|pad| pad.connected).count(), 1);
}

#[test]
fn a_wired_pad_hides_its_paired_bluetooth_entry() {
    let merged = merge_controllers(
        vec![controller(
            "Sony Interactive Entertainment DualSense Wireless Controller",
            None,
            true,
        )],
        vec![controller("DS5", Some("02:00:00:00:00:01"), false)],
    );
    assert_eq!(
        merged.len(),
        1,
        "la même manette ne doit apparaître qu'une fois"
    );
    assert!(merged[0].connected);
    assert!(merged[0].address.is_none());
}

#[test]
fn offline_controllers_have_distinct_selection_keys() {
    let pro = controller("Pro Controller", Some("02:00:00:00:00:02"), false);
    let xbox = controller("Xbox Wireless Controller", Some("AA:BB:CC:DD:EE:FF"), false);
    assert_eq!(controller_key(&pro), "bluetooth:02:00:00:00:00:02");
    assert_ne!(controller_key(&pro), controller_key(&xbox));

    let wired = controller("Zebra Pad", None, true);
    assert_eq!(controller_key(&wired), "path:/dev/input/event-Zebra Pad");
}

#[test]
fn usb_and_bluetooth_share_one_card_and_one_name() {
    let wireless = controller(
        "DualSense Wireless Controller",
        Some("02:00:00:00:00:01"),
        true,
    );
    let mut wired = controller(
        "Sony Interactive Entertainment DualSense Wireless Controller",
        Some("02:00:00:00:00:01"),
        true,
    );
    wired.connection = lapinux_core::model::DeviceLink::Usb;
    wired.path = PathBuf::from("/dev/input/event999");
    let mut names = HashMap::new();
    names.insert(controller_key(&wired), "DS5".to_string());

    let identity = identify(wired.vendor, wired.product, &wired.name);
    assert_eq!(pad_shown_name(&names, &wired, &identity), "DS5");
    let identity = identify(wireless.vendor, wireless.product, &wireless.name);
    assert_eq!(pad_shown_name(&names, &wireless, &identity), "DS5");

    let merged = merge_controllers(vec![wireless, wired], Vec::new());
    assert_eq!(merged.len(), 1, "une seule carte pour le même identifiant");
    assert_eq!(merged[0].connection, lapinux_core::model::DeviceLink::Usb);
    assert_eq!(controller_key(&merged[0]), "bluetooth:02:00:00:00:00:01");
}

#[test]
fn dongle_keeps_the_name_saved_on_its_bluetooth_link() {
    let mut dongle = controller(lapinux_core::i18n::t("pad.radio_2_4ghz"), None, true);
    dongle.connection = lapinux_core::model::DeviceLink::Dongle;
    dongle.vendor = 0x045e;
    dongle.product = 0x028e;
    dongle.path = PathBuf::from("/dev/input/event31");
    let mut bluetooth = controller("Generic Pad", Some("AA:BB:CC:DD:EE:FF"), false);
    bluetooth.vendor = 0x045e;
    bluetooth.product = 0x028e;

    let mut names = HashMap::new();
    names.insert(controller_key(&bluetooth), "Salon".to_string());
    let merged = merge_controllers(vec![dongle], vec![bluetooth]);

    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].address.as_deref(), Some("AA:BB:CC:DD:EE:FF"));
    assert_eq!(controller_key(&merged[0]), "bluetooth:aa:bb:cc:dd:ee:ff");
    let identity = identify(merged[0].vendor, merged[0].product, &merged[0].name);
    assert_eq!(pad_shown_name(&names, &merged[0], &identity), "Salon");
}

#[test]
fn legacy_path_names_survive_the_bluetooth_address_fix() {
    // Une manette sans adresse garde son surnom enregistré par chemin.
    let mut dongle = controller(lapinux_core::i18n::t("pad.radio_2_4ghz"), None, true);
    dongle.connection = lapinux_core::model::DeviceLink::Dongle;
    let mut names = HashMap::new();
    names.insert(
        format!("path:{}", dongle.path.display()),
        "Salon".to_string(),
    );
    let identity = identify(dongle.vendor, dongle.product, &dongle.name);
    assert_eq!(pad_shown_name(&names, &dongle, &identity), "Salon");

    // Dès que la manette est identifiée par son adresse, seule cette clé
    // compte : le numéro de nœud est réutilisé par le noyau, et le
    // réglage laissé là appartenait à une autre manette.
    let mut wireless = dongle.clone();
    wireless.address = Some("AA:BB:CC:DD:EE:FF".into());
    let identity = identify(wireless.vendor, wireless.product, &wireless.name);
    assert_ne!(pad_shown_name(&names, &wireless, &identity), "Salon");

    names.insert(controller_key(&wireless), "Bureau".to_string());
    assert_eq!(pad_shown_name(&names, &wireless, &identity), "Bureau");
}

#[test]
fn connected_dongles_stay_online_and_offline_ones_are_paired() {
    let usb = controller("USB Pad", None, true);
    let mut dongle = controller(lapinux_core::i18n::t("pad.radio_2_4ghz"), None, true);
    dongle.connection = lapinux_core::model::DeviceLink::Dongle;
    let mut offline_dongle = controller(lapinux_core::i18n::t("pad.radio_2_4ghz"), None, false);
    offline_dongle.connection = lapinux_core::model::DeviceLink::Dongle;
    let offline = controller("Pro Controller", Some("02:00:00:00:00:02"), false);
    let all = [usb, dongle, offline_dongle, offline];

    let (online, paired) = split_controller_groups(&all);
    assert_eq!(online.len(), 2);
    assert!(online
        .iter()
        .any(|pad| pad.connection == lapinux_core::model::DeviceLink::Dongle));
    assert_eq!(paired.len(), 2);
    assert!(paired.iter().any(|pad| !pad.connected));
}

#[test]
fn quiet_dongles_leave_the_online_group() {
    let mut dongle = controller(lapinux_core::i18n::t("pad.radio_2_4ghz"), None, true);
    dongle.connection = lapinux_core::model::DeviceLink::Dongle;
    let mut pad = lapinux_input::PadState::default();
    pad.path = dongle.path.clone();
    // Aucune entrée depuis le lancement : le récepteur est branché mais la
    // manette dort, elle n'appartient pas à « En ligne ».
    assert!(dongle_asleep(&dongle, &[pad.clone()], 1_000));
    pad.last_input_unix = Some(1_000 - DONGLE_SILENCE_SECS + 5);
    assert!(!dongle_asleep(&dongle, &[pad.clone()], 1_000));
    pad.last_input_unix = Some(1_000 - DONGLE_SILENCE_SECS - 5);
    assert!(dongle_asleep(&dongle, &[pad], 1_000));
    // Une liaison filaire ne dépend pas de l'activité pour être en ligne.
    let usb = controller("USB Pad", None, true);
    assert!(!dongle_asleep(&usb, &[], 1_000));
}

#[test]
fn steam_puck_pad_is_not_declared_asleep() {
    // hid-steam retire le nœud du pad quand la 2026 s'éteint : son absence
    // est la preuve de la déconnexion, pas le silence des entrées.
    let mut steam = controller("Wireless Steam Controller", None, true);
    steam.vendor = 0x28de;
    steam.product = 0x1304;
    steam.connection = lapinux_core::model::DeviceLink::Dongle;
    let mut pad = lapinux_input::PadState::default();
    pad.path = steam.path.clone();
    assert!(!dongle_asleep(&steam, &[], 1_000));
    assert!(!dongle_asleep(&steam, &[pad.clone()], 1_000));
    pad.last_input_unix = Some(0);
    assert!(!dongle_asleep(&steam, &[pad], 1_000));
}

#[test]
fn custom_pad_names_override_the_model_name() {
    let dualsense = controller(
        "DualSense Wireless Controller",
        Some("02:00:00:00:00:01"),
        true,
    );
    let identity = identify(dualsense.vendor, dualsense.product, &dualsense.name);
    let mut names = HashMap::new();
    assert_eq!(pad_shown_name(&names, &dualsense, &identity), "DualSense");
    names.insert(controller_key(&dualsense), "Salon".to_string());
    assert_eq!(pad_shown_name(&names, &dualsense, &identity), "Salon");
    names.insert(controller_key(&dualsense), "   ".to_string());
    assert_eq!(pad_shown_name(&names, &dualsense, &identity), "DualSense");
}

#[test]
fn list_preview_shows_stick_directions_and_touchpad() {
    let mut pad = lapinux_input::PadState::default();
    pad.axes.insert("left_x".to_string(), 0.8);
    pad.axes.insert("left_y".to_string(), -0.8);
    pad.axes.insert("right_x".to_string(), -0.8);
    pad.axes.insert("right_y".to_string(), 0.0);
    pad.touchpad = Some(lapinux_input::TouchpadState {
        points: vec![lapinux_input::TouchPoint { x: 0.5, y: 0.5 }],
    });
    assert_eq!(
        analog_preview_labels(&pad),
        vec!["G NE".to_string(), "D O".to_string(), "Pavé".to_string()]
    );

    pad.touchpad = None;
    pad.axes.insert("left_x".to_string(), 0.0);
    pad.axes.insert("left_y".to_string(), 0.0);
    pad.axes.insert("right_x".to_string(), 0.0);
    assert!(analog_preview_labels(&pad).is_empty());

    // Une gâchette analogique n'existe que sur son axe : elle doit
    // apparaître dès qu'elle est réellement pressée, sans doublon quand
    // le pilote expose aussi le bouton numérique.
    pad.axes.insert("left_trigger".to_string(), 0.85);
    pad.axes.insert("right_trigger".to_string(), 0.0);
    assert_eq!(analog_trigger_labels(&pad), vec!["LT".to_string()]);
    pad.pressed.push("LT".to_string());
    assert!(analog_trigger_labels(&pad).is_empty());
}

#[test]
fn stick_moves_lead_the_row_preview() {
    let mut pad = lapinux_input::PadState::default();
    pad.axes.insert("left_x".to_string(), 0.9);
    pad.axes.insert("left_y".to_string(), 0.0);
    pad.pressed.push("A".to_string());
    pad.pressed.push("LB".to_string());
    assert_eq!(
        row_preview_labels(
            &pad,
            Some(Brand::PlayStation),
            None,
            FaceButtonProfile::Xbox
        ),
        vec!["G E".to_string(), "Croix".to_string(), "L1".to_string()]
    );
    // Le profil global s'applique aussi à une manette Nintendo : Xbox
    // garde les lettres imprimées, Nintendo permute A/B et X/Y.
    assert_eq!(
        row_preview_labels(&pad, Some(Brand::Nintendo), None, FaceButtonProfile::Xbox),
        vec!["G E".to_string(), "A".to_string(), "L".to_string()]
    );
    assert_eq!(
        row_preview_labels(
            &pad,
            Some(Brand::Nintendo),
            None,
            FaceButtonProfile::Nintendo
        ),
        vec!["G E".to_string(), "B".to_string(), "L".to_string()]
    );
}

#[test]
fn face_profile_replaces_letters_at_their_physical_positions() {
    let mut pad = lapinux_input::PadState::default();
    for label in ["A", "B", "X", "Y"] {
        pad.pressed.push(label.to_string());
    }
    assert_eq!(
        row_preview_labels(
            &pad,
            Some(Brand::Nintendo),
            None,
            FaceButtonProfile::Nintendo
        ),
        vec!["B", "A", "X", "Y"]
    );
    assert_eq!(
        row_preview_labels(&pad, Some(Brand::Nintendo), None, FaceButtonProfile::Xbox),
        vec!["A", "B", "Y", "X"]
    );
}

#[test]
fn xpad_face_letters_light_their_side_of_the_diamond() {
    let mut pad = lapinux_input::PadState::default();
    // Bouton « X » d'une manette Xbox : xpad l'envoie sur BTN_NORTH.
    pad.face.north = true;
    assert_eq!(
        face_active(&pad, Some(Brand::Xbox), None),
        [false, false, false, true],
        "X est à gauche sur la manette"
    );
    // Le pont Steam Controller suit la même convention qu'xpad.
    assert_eq!(
        face_active(&pad, Some(Brand::Steam), None),
        [false, false, false, true]
    );
    // Sur une Switch, le bouton du haut est bien X.
    assert_eq!(
        face_active(&pad, Some(Brand::Nintendo), None),
        [true, false, false, false]
    );
    // Un pad sans marque suit son pilote.
    assert_eq!(
        face_active(&pad, Some(Brand::Generic), Some("xpad")),
        [false, false, false, true]
    );
    assert_eq!(
        face_active(&pad, Some(Brand::Generic), Some("hid-generic")),
        [true, false, false, false]
    );
}

#[test]
fn unknown_vendors_and_consoles_are_named_inconnu() {
    let unknown = identify(0x1234, 0x5678, "Retro Pad");
    assert_eq!(
        console_label(&unknown),
        Some(lapinux_core::i18n::t("controllers.information.unknown"))
    );
    let fallback = identify(0x1234, 0x5678, lapinux_core::i18n::t("pad.radio_2_4ghz"));
    assert_eq!(
        console_label(&fallback),
        Some(lapinux_core::i18n::t("controllers.information.unknown"))
    );
    let clone = identify(0x045e, 0x028e, lapinux_core::i18n::t("pad.radio_2_4ghz"));
    assert_eq!(
        console_label(&clone),
        Some(lapinux_core::i18n::t("controllers.information.unknown"))
    );
    assert_eq!(
        Brand::Generic.label(),
        lapinux_core::i18n::t("controllers.information.unknown")
    );
}

#[test]
fn hidden_dongle_axes_keep_only_the_real_controls() {
    let mut features = vec![
        "bluetooth",
        "sticks",
        "dpad",
        "triggers",
        "hd_vibrations",
        "haptics",
    ];
    hide_dongle_features(&mut features);
    assert_eq!(features, vec!["bluetooth", "dpad"]);
}

#[test]
fn console_names_are_plain_words() {
    let identity = identify(0x054c, 0x0ce6, "DualSense Wireless Controller");
    assert_eq!(console_label(&identity), Some("PlayStation 5"));
    let switch = identify(0x057e, 0x2009, "Pro Controller");
    assert_eq!(console_label(&switch), Some("Nintendo Switch"));
}

/// Halo de l'onglet Téléchargements : ce qui reste à télécharger doit
/// s'effacer devant ce qui est déjà installé. L'anneau de fond était un
/// blanc translucide plus lumineux que l'arc lui-même, d'où l'impression
/// d'un halo déjà presque rempli.
#[test]
fn the_remaining_part_of_the_download_ring_stays_dim() {
    /// Encre perçue d'une couleur : son opacité fois sa luminance.
    fn ink(color: egui::Color32) -> f32 {
        let luma =
            0.2126 * color.r() as f32 + 0.7152 * color.g() as f32 + 0.0722 * color.b() as f32;
        luma * color.a() as f32 / 255.0
    }

    fn walk(
        shape: &egui::Shape,
        track: &mut Option<egui::Color32>,
        arc: &mut Option<egui::Color32>,
        filled_halo: &mut bool,
    ) {
        match shape {
            egui::Shape::Circle(circle)
                if circle.stroke.width > 0.0 && circle.stroke.width < 3.0 =>
            {
                *track = Some(circle.stroke.color);
            }
            egui::Shape::Circle(circle) if circle.radius > 10.0 && circle.fill.a() > 0 => {
                *filled_halo = true;
            }
            egui::Shape::Path(path) if path.stroke.width < 3.0 => {
                if let egui::epaint::ColorMode::Solid(color) = path.stroke.color {
                    *arc = Some(color);
                }
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, track, arc, filled_halo);
                }
            }
            _ => {}
        }
    }

    let ctx = egui::Context::default();
    apply_theme(&ctx);
    let texture = ctx.load_texture(
        "badge-test",
        egui::ColorImage::new([4, 4], egui::Color32::WHITE),
        egui::TextureOptions::LINEAR,
    );
    let badge = NavBadge {
        icon: texture,
        // Une teinte sombre, comme une jaquette brune : l'arc doit
        // malgré tout dominer l'anneau de fond.
        color: egui::Color32::from_rgb(96, 74, 52),
        progress: 0.45,
        running: true,
    };
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let painter = ui.painter().clone();
            paint_download_nav_badge(ui, &painter, egui::pos2(60.0, 20.0), &badge);
        });
    });
    let (mut track, mut arc) = (None, None);
    let mut filled_halo = false;
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut track, &mut arc, &mut filled_halo);
    }
    assert!(
        !filled_halo,
        "aucun halo plein ne doit colorer la part non téléchargée"
    );
    let track = ink(track.expect("anneau de la part restante"));
    let arc = ink(arc.expect("arc de progression"));
    assert!(
        track < 20.0,
        "la part restante reste discrète (encre = {track})"
    );
    assert!(
        arc > track * 3.0,
        "l'arc installé domine la part restante ({arc} contre {track})"
    );
}

/// Pastille loupe + dé : la forme se vérifie sur les tracés rendus, sans
/// regarder l'écran. Fermée, elle porte les deux icônes dans un seul cadre
/// large de deux segments ; ouverte, seul le dé reste, le champ de
/// recherche affichant sa propre loupe.
#[test]
fn the_search_pill_holds_the_magnifier_and_the_die() {
    struct Painted {
        /// Cercles au trait : la loupe, et la face du dé (rect + points).
        magnifiers: usize,
        dies: Vec<egui::Rect>,
        frames: Vec<egui::Rect>,
    }

    fn walk(shape: &egui::Shape, painted: &mut Painted) {
        match shape {
            egui::Shape::Circle(circle) if circle.stroke.width > 0.0 => {
                painted.magnifiers += 1;
            }
            // Le fond du panneau est un rectangle plein, sans trait : seuls
            // les rectangles tracés sont ceux de la pastille et du dé.
            egui::Shape::Rect(rect) if rect.stroke.width > 0.0 => {
                if rect.rect.width() <= 14.0 {
                    painted.dies.push(rect.rect);
                } else {
                    painted.frames.push(rect.rect);
                }
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, painted);
                }
            }
            _ => {}
        }
    }

    let render = |search_open: bool| {
        let ctx = egui::Context::default();
        apply_theme(&ctx);
        let mut painted = Painted {
            magnifiers: 0,
            dies: Vec::new(),
            frames: Vec::new(),
        };
        ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let (open, pick) = search_random_pill(ui, search_open, false);
                assert!(!open && !pick, "aucun clic dans une image sans survol");
            });
        })
        .shapes
        .iter()
        .for_each(|clipped| walk(&clipped.shape, &mut painted));
        painted
    };

    let closed = render(false);
    let frame = closed
        .frames
        .iter()
        .max_by(|a, b| a.width().partial_cmp(&b.width()).unwrap())
        .copied()
        .expect("un cadre pour la pastille");
    assert!(
        (frame.width() - FILTER_W * 2.0).abs() < 0.5,
        "deux segments côte à côte (largeur = {})",
        frame.width()
    );
    assert_eq!(closed.magnifiers, 1, "la loupe du premier segment");
    assert_eq!(closed.dies.len(), 1, "le dé du second segment");
    // Les deux icônes sont sur la même ligne, chacune dans son segment.
    let die = closed.dies[0];
    assert!(
        die.center().y - frame.center().y < 1.0 && frame.center().y - die.center().y < 1.0,
        "le dé reste centré dans la pastille"
    );
    assert!(
        die.center().x > frame.center().x,
        "le dé occupe le segment de droite"
    );

    let open = render(true);
    let frame = open
        .frames
        .iter()
        .max_by(|a, b| a.width().partial_cmp(&b.width()).unwrap())
        .copied()
        .expect("un cadre pour la pastille");
    assert!(
        (frame.width() - FILTER_W).abs() < 0.5,
        "un seul segment quand la recherche est ouverte (largeur = {})",
        frame.width()
    );
    assert_eq!(open.magnifiers, 0, "pas de seconde loupe à côté du champ");
    assert_eq!(open.dies.len(), 1, "le dé reste accessible");
}

/// Recherche dépliée : le champ et le dé doivent rester une seule
/// pastille. Un écart d'un pixel suffit à séparer les deux cadres.
#[test]
fn an_open_search_keeps_the_field_and_the_die_glued() {
    fn walk(shape: &egui::Shape, frames: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Rect(rect) if rect.stroke.width > 0.0 => {
                // Le dé lui-même est un carré de 13 px : seuls les deux
                // cadres de la pastille dépassent 20 px.
                if rect.rect.width() > 20.0 {
                    frames.push(rect.rect);
                }
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, frames);
                }
            }
            _ => {}
        }
    }

    let ctx = egui::Context::default();
    apply_theme(&ctx);
    let mut text = String::new();
    let mut focus = false;
    let mut frames = Vec::new();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                search_field(ui, &mut text, 240.0, &mut focus, false);
                search_random_pill(ui, true, false);
            });
        });
    });
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut frames);
    }
    frames.sort_by(|a, b| a.left().partial_cmp(&b.left()).unwrap());
    assert_eq!(frames.len(), 2, "un cadre pour le champ, un pour le dé");
    let (field, die) = (frames[0], frames[1]);
    assert!(
        (die.left() - field.right()).abs() < 0.5,
        "le dé touche le champ : écart de {} px",
        die.left() - field.right()
    );
    assert!(
        (field.height() - die.height()).abs() < 0.5,
        "même hauteur : {} contre {}",
        field.height(),
        die.height()
    );
}

#[test]
fn default_theme_is_neutral_gray() {
    let ctx = egui::Context::default();
    apply_theme(&ctx);
    let visuals = &ctx.style().visuals;
    assert_eq!(visuals.panel_fill, BACKGROUND());
    assert_eq!(visuals.widgets.inactive.bg_fill, CARD());
    assert_eq!(visuals.widgets.hovered.bg_fill, CARD_HOVER());
    assert!(visuals.dark_mode);
}

#[test]
fn battery_details_are_readable_and_honest() {
    assert_eq!(band_from_percent(75), (70, 79));
    assert_eq!(battery_band_label(Some((70, 79))), "70–79 %");
    assert_eq!(battery_band_label(Some((100, 100))), "100 %");
    assert_eq!(
        format_capacity(BatteryDesign {
            milliamp_hours: 1_560,
            milliwatt_hours: 5_700,
        }),
        "1 560 mAh · 5,7 Wh"
    );
    assert_eq!(
        charge_state_label(ChargeState::Charging),
        lapinux_core::i18n::t("controllers.battery.state_charging")
    );
    assert_eq!(
        charge_state_label(ChargeState::VoltageError),
        lapinux_core::i18n::t("controllers.battery.state_voltage_error")
    );
}

#[test]
fn full_state_keeps_shining_until_unplugged() {
    assert!(battery_charging(Some(ChargeState::Charging)));
    assert!(battery_charging(Some(ChargeState::Full)));
    assert!(!battery_charging(Some(ChargeState::Discharging)));
    // « Full » à 85 % ne doit jamais s'afficher « Pleine ».
    assert_eq!(
        charge_state_label(ChargeState::Full),
        lapinux_core::i18n::t("controllers.battery.state_full")
    );
    assert_eq!(
        charge_state_label(ChargeState::Charging),
        lapinux_core::i18n::t("controllers.battery.state_charging")
    );
}

#[test]
fn charge_sheen_fades_at_both_ends() {
    assert_eq!(sheen_fade(0.0), 0.0);
    assert_eq!(sheen_fade(1.0), 0.0);
    assert!(sheen_fade(0.5) > 0.99);
    assert!(sheen_fade(0.01) < 0.05);
    assert!(sheen_fade(0.99) < 0.05);
}

#[test]
fn charging_ring_draws_the_sheen() {
    fn shape_count(charging: bool) -> usize {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(120.0, 120.0),
        ));
        input.time = Some(1.0);
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                battery_ring(ui, 80, 76.0, charging);
            });
        });
        output.shapes.len()
    }

    assert!(shape_count(true) > shape_count(false));
}

#[test]
fn power_hints_explain_the_ps_wake() {
    assert!(pad_power_hint(PadPowerControl::On {
        pending: true,
        failed: false
    })
    .contains("PS"));
    assert!(pad_power_hint(PadPowerControl::On {
        pending: false,
        failed: false
    })
    .contains("PS"));
    assert_eq!(
        pad_power_hint(PadPowerControl::On {
            pending: false,
            failed: true
        }),
        lapinux_core::i18n::t("controllers.ui.power_on_off")
    );
    assert_eq!(
        pad_power_hint(PadPowerControl::Off {
            armed: false,
            pending: false
        }),
        lapinux_core::i18n::t("controllers.ui.power_off")
    );
}

#[test]
fn power_button_symbols_work_in_the_circle() {
    assert_eq!(
        pad_power_symbol(PadPowerControl::Off {
            armed: false,
            pending: false
        })
        .map(|(glyph, _, _)| glyph),
        Some(ICON_POWER)
    );
    assert_eq!(
        pad_power_symbol(PadPowerControl::Off {
            armed: true,
            pending: false
        })
        .map(|(glyph, _, _)| glyph),
        Some(ICON_CHECK)
    );
    assert!(pad_power_pending(PadPowerControl::On {
        pending: true,
        failed: false
    }));
    assert!(!pad_power_pending(PadPowerControl::On {
        pending: false,
        failed: true
    }));
    assert!(pad_power_symbol(PadPowerControl::Hidden).is_none());
}

#[test]
fn the_detail_sheet_paints_against_the_bottom_of_the_window() {
    let ctx = egui::Context::default();
    apply_theme(&ctx);
    install_fonts(&ctx);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
    let painted = std::cell::Cell::new(egui::Rect::NOTHING);
    // La première image ne sert qu'à mesurer la zone : egui ne la peint
    // qu'à partir de la suivante.
    let frame = || {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.label("Fond");
                });
                let rect = show_detail_sheet(ctx, 1.0, false, |ui| {
                    ui.label("Fiche");
                });
                painted.set(rect);
            },
        )
    };
    frame();
    let output = frame();
    // Le contenu est peint dans la feuille…
    assert!(output.shapes.iter().any(|clipped| {
        matches!(&clipped.shape, egui::Shape::Text(text) if text.galley.text() == "Fiche")
    }));
    // … et la feuille occupe le bas de la fenêtre, sur toute la largeur.
    let rect = painted.get();
    let expected = detail_sheet_rect(screen, 1.0, false);
    assert!(
        (rect.bottom() - screen.bottom()).abs() < 1.0,
        "collée au bas (bas = {})",
        rect.bottom()
    );
    assert!(
        (rect.width() - expected.width()).abs() < 1.0,
        "toute la largeur (largeur = {})",
        rect.width()
    );
    assert!(
        (rect.left() - expected.left()).abs() < 1.0,
        "marges latérales (x = {})",
        rect.left()
    );
    assert!(
        rect.top() > 200.0,
        "la bibliothèque reste visible (haut = {})",
        rect.top()
    );
}

#[test]
fn the_sheet_stays_half_hidden_midway() {
    let ctx = egui::Context::default();
    apply_theme(&ctx);
    install_fonts(&ctx);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
    let rect = std::cell::Cell::new(egui::Rect::NOTHING);
    let frame = || {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.label("Fond");
                });
                rect.set(show_detail_sheet(ctx, 0.5, false, |ui| {
                    ui.label("Fiche");
                }));
            },
        )
    };
    frame();
    frame();
    let expected = detail_sheet_rect(screen, 0.5, false);
    assert!(
        (rect.get().top() - expected.top()).abs() < 1.0,
        "à mi-course, la fiche dépasse sous la fenêtre (haut = {} / {})",
        rect.get().top(),
        expected.top()
    );
    assert!(rect.get().bottom() > screen.bottom());
}

#[test]
fn the_daemon_state_tells_a_stale_build_from_a_fresh_one() {
    let build = |stamp| DaemonBuild {
        stamp,
        version: "0.1.0".into(),
        started: 1_700_000_000,
    };
    assert_eq!(daemon_state(None, Some(100)), DaemonState::Absent);
    assert_eq!(
        daemon_state(Some(&build(100)), Some(100)),
        DaemonState::Current
    );
    assert_eq!(
        daemon_state(Some(&build(200)), Some(100)),
        DaemonState::Current
    );
    assert_eq!(
        daemon_state(Some(&build(99)), Some(100)),
        DaemonState::Stale
    );
    // Un daemon d'avant l'empreinte répond zéro : il est plus ancien.
    assert_eq!(daemon_state(Some(&build(0)), Some(100)), DaemonState::Stale);
    // Sans date de binaire, on ne condamne pas un daemon qui répond.
    assert_eq!(daemon_state(Some(&build(1)), None), DaemonState::Current);
    assert_eq!(
        DaemonState::Absent.label(),
        lapinux_core::i18n::t("settings.system.state_stopped")
    );
    assert_eq!(
        DaemonState::Stale.label(),
        lapinux_core::i18n::t("settings.system.state_stale")
    );
    assert_eq!(DaemonState::Stale.color(), WARN);
}

#[test]
fn a_build_stamp_is_shown_as_a_date() {
    assert_eq!(
        build_stamp_label(0),
        lapinux_core::i18n::t("controllers.information.unknown_band")
    );
    // 1 700 000 000 s = 14/11/2023 ; seule la date est vérifiée, l'heure
    // dépend du fuseau de la machine.
    let label = build_stamp_label(1_700_000_000_000);
    assert!(label.starts_with("14/11/2023"), "{label}");
}

#[test]
fn the_collapsed_sheet_is_a_single_low_row() {
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1360.0, 860.0));
    let compact = detail_sheet_rect(screen, 1.0, false);
    assert!((compact.height() - SHEET_COMPACT_HEIGHT).abs() < 0.5);
    // La grille garde les deux tiers de la fenêtre au-dessus.
    assert!(
        compact.top() > screen.height() * 0.6,
        "haut = {}",
        compact.top()
    );
    // Dépliée, la fiche monte sans jamais couvrir toute la fenêtre.
    let expanded = detail_sheet_rect(screen, 1.0, true);
    assert!(expanded.height() > compact.height() + 100.0);
    assert!(expanded.top() > 96.0);
    // Sur une fenêtre basse, la fiche repliée reste dans l'écran.
    let low = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 260.0));
    let rect = detail_sheet_rect(low, 1.0, false);
    assert!((rect.bottom() - low.bottom()).abs() < 0.5);
    assert!(rect.top() >= 0.0, "haut = {}", rect.top());
}

#[test]
fn the_sheet_panel_stays_centred_whatever_the_row_asks() {
    let ctx = egui::Context::default();
    apply_theme(&ctx);
    install_fonts(&ctx);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1345.0, 874.0));
    let expected = detail_sheet_rect(screen, 1.0, false);
    let mut panel = egui::Rect::NOTHING;
    for _ in 0..3 {
        let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ctx| {
                    show_detail_sheet(ctx, 1.0, false, |ui| {
                        // Une ligne de fiche, avec des textes trop longs pour
                        // leurs colonnes.
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.horizontal_top(|ui| {
                            let columns = compact_columns(ui.available_width());
                            let height = ui.available_height();
                            ui.allocate_ui_with_layout(
                                egui::vec2(SHEET_CONTROLS_WIDTH, height),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    ui.set_width(SHEET_CONTROLS_WIDTH);
                                    ui.allocate_exact_size(
                                        egui::vec2(26.0, 26.0),
                                        egui::Sense::click(),
                                    );
                                },
                            );
                            ui.add_space(SHEET_CONTROLS_GAP);
                            // L'encart d'identité, jaquette comprise : son
                            // contenu ne doit pas l'élargir.
                            let card = columns.identity - 20.0;
                            let text_width = (card - SHEET_COVER_WIDTH - 10.0).max(40.0);
                            egui::Frame::none()
                                .fill(CARD())
                                .rounding(egui::Rounding::same(10.0))
                                .inner_margin(egui::Margin::same(10.0))
                                .show(ui, |ui| {
                                    ui.set_width(card);
                                    ui.horizontal_top(|ui| {
                                        ui.spacing_mut().item_spacing.x = 10.0;
                                        ui.allocate_exact_size(
                                            egui::vec2(SHEET_COVER_WIDTH, 87.0),
                                            egui::Sense::hover(),
                                        );
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(text_width, height),
                                            egui::Layout::top_down(egui::Align::Min),
                                            |ui| {
                                                ui.set_width(text_width);
                                                ui.style_mut().wrap_mode =
                                                    Some(egui::TextWrapMode::Truncate);
                                                ui.label(
                                                    "Un titre de jeu interminable qui ne doit pas élargir la fiche",
                                                );
                                                ui.label(
                                                    "Installé · Proton · Compte personnel · Steamworks · joué 128 h",
                                                );
                                            },
                                        );
                                    });
                                });
                            ui.add_space(columns.identity_gap);
                            ui.allocate_ui_with_layout(
                                egui::vec2(columns.middle, height),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    ui.set_width(columns.middle);
                                    ui.label(lapinux_core::i18n::t("achievements.title"));
                                },
                            );
                        });
                    });
                },
            );
        fn visit(shape: &egui::Shape, panel: &mut egui::Rect) {
            match shape {
                // Le panneau apparaît en fondu : sa couleur est mise à
                // l'échelle. L'ombre, elle, est noire. L'encart d'identité
                // est aussi une carte opaque : sa largeur (340 px au plus)
                // le distingue du panneau, large de plus de 600 px.
                egui::Shape::Rect(rect)
                    if rect.fill.r() + rect.fill.g() > 0 && rect.rect.width() > 600.0 =>
                {
                    *panel = rect.rect;
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        visit(shape, panel);
                    }
                }
                _ => {}
            }
        }
        for clipped in &output.shapes {
            visit(&clipped.shape, &mut panel);
        }
    }
    assert!(panel.is_positive(), "panneau peint");
    assert!(
        (panel.left() - expected.left()).abs() < 1.0,
        "gauche = {} / {}",
        panel.left(),
        expected.left()
    );
    assert!(
        (panel.right() - expected.right()).abs() < 1.0,
        "droite = {} / {}",
        panel.right(),
        expected.right()
    );
}

#[test]
fn the_sheet_keeps_a_margin_on_both_sides() {
    for width in [900.0_f32, 1360.0, 1920.0, 3840.0] {
        let margin = sheet_margin_x(width);
        assert!(margin >= 24.0 && margin <= 120.0, "{margin} px à {width}");
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 900.0));
        let sheet = detail_sheet_rect(screen, 1.0, false);
        assert!(
            (sheet.left() - margin).abs() < 0.5,
            "gauche = {}",
            sheet.left()
        );
        assert!(
            (screen.right() - sheet.right() - margin).abs() < 0.5,
            "droite = {}",
            screen.right() - sheet.right()
        );
        // La fiche ne prend jamais toute la largeur de la fenêtre.
        assert!(sheet.width() < width - 40.0);
    }
}

/// Dépliée, la colonne des fonctionnalités prend le bord droit et laisse
/// un corps encore lisible à sa gauche ; l'espace libre entre l'encart et
/// les succès reçoit la colonne de faits, sans étirer les succès.
/// Dépliée et remontée, la fiche ne doit pas s'écraser : le corps et la
/// colonne des fonctionnalités reçoivent la hauteur restante. Une
/// allocation de hauteur nulle les rognait, laissant la fiche vide en bas.
/// La fenêtre d'origine : 1 372 px de large. La fiche y perd ses deux
/// marges puis sa marge intérieure ; à cette largeur, la colonne des
/// fonctionnalités doit monter — c'est ce que l'ancien seuil de 1 200 px
/// ne faisait jamais, la fiche mesurant 1 190 px.
/// Les compteurs passent dans la bande de tags : joueurs connectés, avis
/// déposés et part d'avis positifs.
#[test]
fn the_tags_count_players_votes_and_approval() {
    let mut info = SteamGameInfo::default();
    info.account.players_now = Some(12_345);
    info.store.review_count = Some(4_567);
    let details = SteamDetails {
        review_percent: Some(94),
        ..Default::default()
    };
    let facts = sheet_facts(&details, Some(&info), None);
    let values: Vec<String> = facts.iter().map(|(_, value)| value.clone()).collect();
    assert!(
        values.iter().any(|value| value == "12\u{202f}345"),
        "{values:?}"
    );
    assert!(
        values.iter().any(|value| value == "4\u{202f}567"),
        "{values:?}"
    );
    assert!(values.iter().any(|value| value == "94 %"), "{values:?}");
    // Le total des recommandations prend le relais quand le nombre d'avis
    // n'est pas publié.
    info.store.review_count = None;
    info.store.recommendations = Some(42_000);
    let facts = sheet_facts(&details, Some(&info), None);
    assert!(facts.iter().any(|(_, value)| value == "42\u{202f}000"));
}

/// Le nuage n'apparaît que sur un jeu qui synchronise ses sauvegardes.
#[test]
fn the_cloud_mark_follows_the_steam_category() {
    let mut info = SteamGameInfo::default();
    assert!(!cloud_saves(Some(&info)));
    assert!(!cloud_saves(None));
    info.store.features = vec![lapinux_core::model::SteamFeature {
        id: 23,
        label: "Steam Cloud".into(),
    }];
    assert!(cloud_saves(Some(&info)));
    // Une catégorie ajoutée par la boutique se lit au libellé.
    info.store.features = vec![lapinux_core::model::SteamFeature {
        id: 999,
        label: "Cloud saves".into(),
    }];
    assert!(cloud_saves(Some(&info)));
}

/// La bande de tags dit ce que le compte garde et si la copie locale suit.
#[test]
fn the_cloud_tag_counts_files_and_flags_a_stale_copy() {
    let mut report = lapinux_core::model::CloudReport::default();
    report.saves.files = vec![lapinux_core::model::CloudFile {
        name: "GameSave.sav".into(),
        size: 3_122_430,
        modified: 1_700_000_000,
        local: Some(1_700_000_000),
        local_resolved: true,
        sha: vec![1; 20],
        local_sha: Some(vec![1; 20]),
        ..Default::default()
    }];
    let facts = sheet_facts(&SteamDetails::default(), None, Some(&report));
    let cloud = facts
        .iter()
        .find(|(label, _)| *label == lapinux_core::i18n::t("stores.capability.cloud"))
        .expect("tag cloud");
    assert!(
        cloud.1.starts_with(&lapinux_core::i18n::t_fmt(
            "sheet_saves.files_count",
            &["1".to_string()]
        )),
        "{}",
        cloud.1
    );
    assert!(
        cloud
            .1
            .contains(lapinux_core::i18n::t("protons.up_to_date")),
        "{}",
        cloud.1
    );
    assert!(cloud.1.contains("sync"), "{}", cloud.1);
    assert!(
        cloud
            .1
            .contains(lapinux_core::i18n::t("saves.on_this_pc_plain")),
        "{}",
        cloud.1
    );
    // Les deux informations forment un seul tag, sans doublon.
    assert!(!facts.iter().any(|(label, _)| *label == "Dernière sync"));
    assert_eq!(
        cloud_sync_label(&report),
        lapinux_core::i18n::t("protons.up_to_date")
    );

    // Fichier absent du préfixe, puis plus ancien que le cloud.
    report.saves.files[0].local = None;
    report.saves.files[0].local_sha = None;
    assert_eq!(
        cloud_sync_label(&report),
        lapinux_core::i18n::t("sheet_saves.to_fetch")
    );
    report.saves.files[0].local = Some(1_600_000_000);
    report.saves.files[0].local_sha = Some(vec![2; 20]);
    assert_eq!(
        cloud_sync_label(&report),
        lapinux_core::i18n::t("sheet_saves.conflict")
    );
}

/// Un jeu qui annonce le cloud sans rien dedans le dit explicitement.
#[test]
fn a_game_without_cloud_files_says_so() {
    let mut info = SteamGameInfo::default();
    info.store.features = vec![lapinux_core::model::SteamFeature {
        id: 23,
        label: "Steam Cloud".into(),
    }];
    let facts = sheet_facts(
        &SteamDetails::default(),
        Some(&info),
        Some(&lapinux_core::model::CloudReport::default()),
    );
    assert!(facts.iter().any(|(label, value)| *label
        == lapinux_core::i18n::t("stores.capability.cloud")
        && value == lapinux_core::i18n::t("game_sheet.cloud_no_saves")));
}

#[test]
fn figures_are_grouped_by_thousands() {
    assert_eq!(grouped(7), "7");
    assert_eq!(grouped(999), "999");
    assert_eq!(grouped(1_234), "1\u{202f}234");
    assert_eq!(grouped(1_234_567), "1\u{202f}234\u{202f}567");
}

/// La bande de tags ferme la fiche sur une seule ligne, dans sa largeur.
#[test]
fn the_tags_band_stays_on_one_line_inside_the_sheet() {
    let ctx = egui::Context::default();
    apply_theme(&ctx);
    install_fonts(&ctx);
    let facts = vec![
        (lapinux_core::i18n::t("game_sheet.fact_age"), "PEGI 18".to_string()),
        (
            lapinux_core::i18n::t("collections.developer"),
            "Turtle Rock Studios".to_string(),
        ),
        (
            lapinux_core::i18n::t("collections.publisher"),
            "Warner Bros. Games".to_string(),
        ),
        ("Prix", "59,99 €".to_string()),
        ("Sortie", "2021".to_string()),
    ];
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1372.0, 900.0));
    let width = detail_sheet_rect(screen, 1.0, true).width() - 44.0;
    let mut band = egui::Rect::NOTHING;
    let mut chips = egui::Rect::NOTHING;
    for _ in 0..3 {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                egui::Area::new(egui::Id::new("tags-band"))
                    .fixed_pos(egui::pos2(0.0, 0.0))
                    .show(ctx, |ui| {
                        ui.set_width(width);
                        sheet_tags(ui, &facts);
                        band = ui.min_rect();
                    });
            },
        );
        // Les puces de la bande, pour vérifier qu'elles sont centrées.
        fn walk(shape: &egui::Shape, chips: &mut egui::Rect) {
            match shape {
                // Les puces sont les seuls rectangles de la hauteur de la
                // bande : leur teinte, elle, dépend du thème.
                egui::Shape::Rect(rect)
                    if (rect.rect.height() - SHEET_TAGS_HEIGHT).abs() < 0.5
                        && rect.rect.width() > 20.0 =>
                {
                    let rect = rect.rect;
                    chips.min.x = chips.min.x.min(rect.left());
                    chips.max.x = chips.max.x.max(rect.right());
                    chips.min.y = chips.min.y.min(rect.top());
                    chips.max.y = chips.max.y.max(rect.bottom());
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, chips);
                    }
                }
                _ => {}
            }
        }
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut chips);
        }
    }
    assert!(band.width() <= width + 0.5, "bande trop large : {band:?}");
    assert!(
        band.height() <= SHEET_TAGS_HEIGHT + 8.0,
        "la bande passe sur deux lignes : {band:?}"
    );
    // Les tags sont centrés : autant de place devant le premier que
    // derrière le dernier.
    assert!(chips.width() > 100.0, "aucune puce peinte : {chips:?}");
    let left = chips.left();
    let right = width - chips.right();
    assert!(
        (left - right).abs() < 1.5,
        "bande décentrée : {left} devant, {right} derrière ({chips:?})"
    );
}

#[test]
fn the_topbar_clock_keeps_its_width_when_digits_change() {
    let ctx = egui::Context::default();
    install_fonts(&ctx);
    apply_theme(&ctx);
    let mut widths = Vec::new();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            for time in ["00:00:00", "11:11:11", "23:54:09", "18:08:58"] {
                widths.push(ui.fonts(|fonts| {
                    fonts
                        .layout_no_wrap(time.to_string(), topbar_clock_font(), TEXT())
                        .size()
                        .x
                }));
            }
        });
    });
    assert!(widths.iter().all(|width| *width > 80.0), "{widths:?}");
    assert!(
        widths
            .windows(2)
            .all(|pair| (pair[0] - pair[1]).abs() < 0.01),
        "largeur variable : {widths:?}"
    );
    assert!(
        widths
            .iter()
            .all(|width| *width + 8.0 <= TOPBAR_CLOCK_WIDTH),
        "heure rognée : {widths:?}"
    );
}

#[test]
fn the_compact_row_keeps_its_columns_inside_the_sheet() {
    for width in (700..=2000).step_by(50) {
        let width = width as f32;
        let columns = compact_columns(width);
        assert!(columns.middle >= 120.0, "succès trop étroits à {width}");
        assert!((300.0..=400.0).contains(&columns.identity));
        // Les colonnes remplissent la fiche : la place que les succès ne
        // prennent pas devant eux s'ouvre entre l'encart et eux.
        let used = SHEET_CONTROLS_WIDTH
            + SHEET_CONTROLS_GAP
            + columns.identity
            + columns.identity_gap
            + columns.middle;
        assert!((used - width).abs() < 0.5, "débordement à {width} : {used}");
        assert!(columns.identity_gap >= 18.0, "encart collé à {width}");
    }
    // À la largeur courante, les succès gardent une colonne mesurée : plus
    // large, la bande d'icônes s'étalait sans montrer plus de succès.
    let wide = compact_columns(1284.0);
    assert!(wide.middle <= SHEET_MIDDLE_MAX, "{}", wide.middle);
    assert!(
        wide.middle <= 1284.0 * SHEET_MIDDLE_SHARE + 0.5,
        "{}",
        wide.middle
    );
    assert!(wide.middle > wide.identity_gap / 2.0, "{}", wide.middle);
    // La jaquette des grandes fiches prend la hauteur de la carte, et la
    // colonne de texte garde de quoi porter le nom et ses trois actions.
    let cover = (COVER_BESIDE_HEIGHT * SHEET_COVER_WIDTH / 87.0).round();
    assert!(
        wide.identity - 20.0 - cover - 10.0 >= 176.0,
        "jaquette {cover} dans {}",
        wide.identity
    );
}

/// Quand la recherche s'ouvre, le dé reste collé au champ : les deux
/// forment une seule pastille, angles droits de part et d'autre.
#[test]
fn the_search_field_and_the_die_share_one_pill() {
    let ctx = egui::Context::default();
    apply_theme(&ctx);
    install_fonts(&ctx);
    let mut text = String::new();
    let mut focus = false;
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600.0, 120.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // L'écart se retire avant le champ : il est réservé
                    // au moment où l'élément est posé.
                    ui.spacing_mut().item_spacing.x = 0.0;
                    search_field(ui, &mut text, 240.0, &mut focus, false);
                    let _ = search_random_pill(ui, true, false);
                });
            });
        },
    );
    fn walk(shape: &egui::Shape, out: &mut Vec<egui::epaint::RectShape>) {
        match shape {
            egui::Shape::Rect(rect) if rect.fill == CARD() => out.push(rect.clone()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, out);
                }
            }
            _ => {}
        }
    }
    let mut pills = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut pills);
    }
    assert_eq!(pills.len(), 2, "le champ et le dé, rien d'autre");
    let (field, die) = (&pills[0], &pills[1]);
    assert!(
        (die.rect.left() - field.rect.right()).abs() < 0.6,
        "le dé touche le champ : {:?} puis {:?}",
        field.rect,
        die.rect
    );
    assert_eq!(field.rounding.ne, 0.0, "coin droit du champ carré");
    assert_eq!(field.rounding.se, 0.0, "coin droit du champ carré");
    assert_eq!(die.rounding.nw, 0.0, "coin gauche du dé carré");
    assert_eq!(die.rounding.sw, 0.0, "coin gauche du dé carré");
}

/// Le panneau fixe porte son titre et son sous-titre, dessine son contenu
/// et se referme par Échap.
#[test]
fn the_fixed_panel_holds_its_title_and_closes_on_escape() {
    let ctx = egui::Context::default();
    apply_theme(&ctx);
    install_fonts(&ctx);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 700.0));
    let mut close = false;
    let run = |events: Vec<egui::Event>, close: &mut bool| -> Vec<String> {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            },
            |ctx| {
                *close = fixed_panel(
                    ctx,
                    "test-panel",
                    "Titre du panneau",
                    "sous-titre du panneau",
                    egui::vec2(520.0, 400.0),
                    |ui| {
                        ui.label("contenu du panneau");
                    },
                );
            },
        );
        fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut seen = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut seen);
        }
        seen
    };
    run(Vec::new(), &mut close);
    let seen = run(Vec::new(), &mut close);
    for expected in [
        "Titre du panneau",
        "sous-titre du panneau",
        "contenu du panneau",
    ] {
        assert!(
            seen.iter().any(|text| text == expected),
            "« {expected} » manque : {seen:?}"
        );
    }
    assert!(!close, "un panneau frais reste ouvert");
    // Échap referme le panneau, sans rien laisser d'autre le faire.
    run(
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }],
        &mut close,
    );
    assert!(close, "Échap referme le panneau");
}

/// Un panneau sans bandeau — les réglages d'un jeu, désormais aussi les
/// réglages généraux — garde une croix cliquable dans son coin.
#[test]
fn a_headerless_panel_closes_with_its_corner_cross() {
    let ctx = egui::Context::default();
    apply_theme(&ctx);
    install_fonts(&ctx);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 700.0));
    let panel = egui::Rect::from_center_size(screen.center(), egui::vec2(520.0, 400.0));
    let target = egui::pos2(panel.right() - 24.0, panel.top() + 23.0);
    let mut close = false;
    let frame = |events: Vec<egui::Event>, close: &mut bool| {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            },
            |ctx| {
                *close = fixed_panel_with_scroll(
                    ctx,
                    "test-headerless-panel",
                    "Sans bandeau",
                    "",
                    egui::vec2(520.0, 400.0),
                    true,
                    false,
                    64,
                    |ui| {
                        ui.label("contenu");
                    },
                );
            },
        );
    };
    frame(Vec::new(), &mut close);
    frame(vec![egui::Event::PointerMoved(target)], &mut close);
    frame(
        vec![egui::Event::PointerButton {
            pos: target,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        }],
        &mut close,
    );
    frame(
        vec![egui::Event::PointerButton {
            pos: target,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }],
        &mut close,
    );
    assert!(close, "la croix du panneau sans bandeau doit fermer");
}

#[test]
fn the_identity_card_frames_the_cover_name_and_actions() {
    let ctx = egui::Context::default();
    apply_theme(&ctx);
    install_fonts(&ctx);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 420.0));
    /// Peint l'encart et rend sa carte et les textes qu'il contient.
    fn paint(
        ctx: &egui::Context,
        screen: egui::Rect,
        identity: f32,
        height: f32,
        buttons: CardButtons,
    ) -> (egui::Rect, Vec<(String, egui::Rect)>) {
        fn walk(shape: &egui::Shape, card: &mut egui::Rect, texts: &mut Vec<(String, egui::Rect)>) {
            match shape {
                // L'encart partage sa couleur avec les boutons : c'est la
                // plus grande surface de cette teinte.
                egui::Shape::Rect(rect)
                    if rect.fill == CARD() && rect.rect.width() > card.width() =>
                {
                    *card = rect.rect
                }
                egui::Shape::Text(text) => texts.push((
                    text.galley.text().to_string(),
                    egui::Rect::from_min_size(text.pos, text.galley.rect.size()),
                )),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, card, texts);
                    }
                }
                _ => {}
            }
        }
        let mut output = None;
        for _ in 0..2 {
            output = Some(ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        identity_card(
                            ui,
                            identity,
                            height,
                            440,
                            "8AM",
                            &None,
                            &None,
                            false,
                            None,
                            Some(false),
                            false,
                            None,
                            true,
                            100,
                            "Installé · Proton · compte personnel · appid 440",
                            "Joué 128 h",
                            "Total Steam · 128 h\nLinux · 90 h\nWindows · 38 h",
                            "4 Go",
                            true,
                            None,
                            &buttons,
                            false,
                        );
                    });
                },
            ));
        }
        let output = output.expect("une image peinte");
        let mut card = egui::Rect::NOTHING;
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut card, &mut texts);
        }
        (card, texts)
    }

    let labels = |texts: &[(String, egui::Rect)]| -> Vec<String> {
        texts.iter().map(|(text, _)| text.clone()).collect()
    };

    // Installé : jouer, régler, désinstaller — les deux icônes dessinées.
    let (card, texts) = paint(
        &ctx,
        screen,
        340.0,
        SHEET_COMPACT_HEIGHT,
        CardButtons::Installed { update: false },
    );
    assert!(card.is_positive(), "encart peint");
    assert!(
        (card.width() - 340.0).abs() < 0.5,
        "largeur = {}",
        card.width()
    );
    // La carte s'arrête sous la jaquette : l'espace libre reste à
    // disposition sous l'encart.
    assert!(
        (card.height() - (COVER_BESIDE_HEIGHT + 20.0)).abs() < 1.0,
        "hauteur = {}",
        card.height()
    );
    let label_rect = |texts: &[(String, egui::Rect)], label: &str| -> egui::Rect {
        texts
            .iter()
            .filter(|(text, _)| text == label)
            // Le nom s'écrit aussi au centre de la jaquette quand le jeu
            // n'a pas d'image : le vrai libellé est le plus à droite.
            .max_by(|left, right| left.1.left().total_cmp(&right.1.left()))
            .map(|(_, rect)| *rect)
            .unwrap_or_else(|| panic!("« {label} » absent : {:?}", labels(texts)))
    };
    let name_rect = label_rect(&texts, "8AM");
    let play_text = format!("▶  {}", lapinux_core::i18n::t("library.action.play"));
    let play = label_rect(&texts, &play_text);
    // La jaquette monte jusqu'à la hauteur de la carte : le nom et les
    // actions commencent bien plus loin qu'avec la petite jaquette du pied
    // de carte (58 px de large).
    let cover = name_rect.left() - card.left() - 20.0;
    let expected = (COVER_BESIDE_HEIGHT * SHEET_COVER_WIDTH / 87.0).round();
    assert!(
        (cover - expected).abs() < 1.5,
        "jaquette de {cover} px au lieu de {expected}"
    );
    assert!(cover > SHEET_COVER_WIDTH, "jaquette {cover} px");
    // Les actions se rangent à droite de la jaquette, sous le nom : leur
    // bloc commence au bord de la colonne de texte, qu'elles remplissent,
    // au lieu d'être rejetées au bord droit de l'encart. Le libellé de
    // « Jouer » est centré dans son bouton : c'est le bord du bouton qui
    // dit où commence le bloc.
    // Les boutons à icône sont des carrés de la hauteur de la rangée : ils
    // s'alignent sur « Jouer » au lieu de flotter plus petits à côté.
    assert_eq!(CARD_GLYPH_SIZE, CARD_ROW_HEIGHT);
    let text_column = (340.0 - 20.0) - cover - 10.0;
    let play_width =
        (text_column.min(CARD_ACTIONS_WIDTH) - CARD_GLYPH_SIZE - CARD_GLYPH_SIZE - 2.0 * 8.0)
            .max(72.0);
    let play_left = play.center().x - play_width / 2.0;
    assert!(
        play_left >= name_rect.left() && play_left - name_rect.left() < 40.0,
        "action à {play_left} pour un nom à {}",
        name_rect.left()
    );
    assert!(
        play.top() > name_rect.bottom(),
        "les actions passent sous le nom"
    );
    // Le temps de jeu et le poids tiennent une ligne à part, sous le nom et
    // au-dessus des actions, chacun derrière son pictogramme — l'encart ne
    // se contente pas de les accoler aux icônes de provenance.
    let played_rect = label_rect(&texts, "Joué 128 h");
    let size_rect = label_rect(&texts, "4 Go");
    for (label, rect) in [("heures", played_rect), ("poids", size_rect)] {
        assert!(
            rect.top() > name_rect.bottom(),
            "les {label} passent sous le nom"
        );
        assert!(
            rect.bottom() <= play.top(),
            "les {label} restent au-dessus des actions"
        );
    }
    assert!(
        played_rect.right() <= size_rect.left(),
        "le poids suit les heures sur la même ligne"
    );
    // Et elles sont collées au bas de la jaquette, pas au bas de la carte.
    let cover_bottom = card.top() + 10.0 + COVER_BESIDE_HEIGHT;
    assert!(
        play.bottom() < cover_bottom && cover_bottom - play.bottom() < 16.0,
        "action à {} pour un bas de jaquette à {cover_bottom}",
        play.bottom()
    );
    for (text, rect) in &texts {
        assert!(
            card.contains_rect(*rect),
            "« {text} » sort de l'encart {card:?} / {rect:?}"
        );
    }
    // Les icônes d'origine remplacent le texte d'état : aucune ligne d'appid.
    assert!(
        !labels(&texts).iter().any(|text| text.contains("appid")),
        "l'encart ne doit plus écrire l'appid : {:?}",
        labels(&texts)
    );

    // À télécharger : le poids du jeu sur le bouton, et les contenus.
    let (card, texts) = paint(
        &ctx,
        screen,
        250.0,
        128.0,
        CardButtons::Download {
            appid: 440,
            size: Some(10_100_000_000),
            dlc: 3,
            versions: Vec::new(),
            mobile: Vec::new(),
            chosen: 0,
            family: Vec::new(),
            family_chosen: 0,
            dlc_names: Vec::new(),
            steam: true,
        },
    );
    assert!(
        (card.width() - 250.0).abs() < 0.5,
        "largeur = {}",
        card.width()
    );
    assert!(
        (card.height() - 128.0).abs() < 0.5,
        "hauteur = {}",
        card.height()
    );
    let download = lapinux_core::i18n::t_args(
        "game_sheet.download_size",
        &[("size", "10,1 Go")],
    );
    assert!(
        labels(&texts).iter().any(|text| text == &download),
        "{:?}",
        labels(&texts)
    );
    assert!(labels(&texts).iter().any(|text| text == "DLC · 3"));
    let play_text = format!("▶  {}", lapinux_core::i18n::t("library.action.play"));
    assert!(!labels(&texts).iter().any(|text| *text == play_text));
    // Le bouton des contenus finit au bord droit de l'encart. Le libellé
    // est centré dans son bouton : c'est le bord du bouton, et non celui
    // du texte, qui dit où s'arrête la rangée.
    let dlc = texts
        .iter()
        .find(|(text, _)| text == "DLC · 3")
        .map(|(_, rect)| *rect)
        .expect("bouton des contenus");
    let dlc_right = dlc.center().x + DLC_BUTTON / 2.0;
    assert!(
        card.right() - dlc_right < 26.0,
        "le pied s'aligne à droite (bouton jusqu'à {dlc_right} dans {card:?})"
    );
    for (text, rect) in &texts {
        assert!(
            card.contains_rect(*rect),
            "« {text} » sort de l'encart {card:?} / {rect:?}"
        );
    }

    // Mobile uniquement : le bouton d'installation PC disparaît ; le menu
    // Android/iOS reste consultable et la fiche l'annonce clairement.
    let (card, texts) = paint(
        &ctx,
        screen,
        340.0,
        SHEET_COMPACT_HEIGHT,
        CardButtons::Mobile {
            appid: 440,
            versions: vec![
                GameVersion {
                    id: "android",
                    label: "Android",
                    size: None,
                },
                GameVersion {
                    id: "ios",
                    label: "iOS",
                    size: None,
                },
            ],
        },
    );
    assert!(
        labels(&texts)
            .iter()
            .any(|text| *text == lapinux_core::i18n::t("game_sheet.mobile_only")),
        "{:?}",
        labels(&texts)
    );
    // Le préfixe du bouton de téléchargement, dans la langue active : la
    // comparaison ne doit pas dépendre du mot français.
    let download_prefix = lapinux_core::i18n::t("game_sheet.download_size")
        .split('{')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    assert!(
        !labels(&texts)
            .iter()
            .any(|text| text.starts_with(&download_prefix)),
        "un jeu mobile ne doit pas proposer de téléchargement PC : {:?}",
        labels(&texts)
    );
    for (text, rect) in &texts {
        assert!(
            card.contains_rect(*rect),
            "« {text} » sort de l'encart {card:?} / {rect:?}"
        );
    }

    // En cours : le pourcentage remplace le bouton, la croix annule.
    let (card, texts) = paint(
        &ctx,
        screen,
        340.0,
        SHEET_COMPACT_HEIGHT,
        CardButtons::Progress {
            fraction: 0.42,
            job: "jeu-1".into(),
            appid: 440,
        },
    );
    assert!(labels(&texts).iter().any(|text| text == "42 %"));
    assert!(!labels(&texts)
        .iter()
        .any(|text| text == lapinux_core::i18n::t("workshop.download")));
    for (text, rect) in &texts {
        assert!(
            card.contains_rect(*rect),
            "« {text} » sort de l'encart {card:?} / {rect:?}"
        );
    }

    // Le point qui fâchait : la jaquette ne doit pas bouger d'un état à
    // l'autre. Même hauteur, même place, à l'instant où un téléchargement
    // démarre.
    for (state, buttons) in [
        (
            lapinux_core::i18n::t("protons.installed"),
            CardButtons::Installed { update: false },
        ),
        (
            "à télécharger",
            CardButtons::Download {
                appid: 440,
                size: Some(10_100_000_000),
                dlc: 0,
                versions: Vec::new(),
                mobile: Vec::new(),
                chosen: 0,
                family: Vec::new(),
                family_chosen: 0,
                dlc_names: Vec::new(),
                steam: true,
            },
        ),
        (
            "en cours",
            CardButtons::Progress {
                fraction: 0.42,
                job: "jeu-1".into(),
                appid: 440,
            },
        ),
    ] {
        let (card, texts) = paint(&ctx, screen, 340.0, SHEET_COMPACT_HEIGHT, buttons);
        assert!(
            (card.height() - (COVER_BESIDE_HEIGHT + 20.0)).abs() < 1.0,
            "« {state} » : carte de {} px",
            card.height()
        );
        let cover = label_rect(&texts, "8AM").left() - card.left() - 20.0;
        assert!(
            (cover - expected).abs() < 1.5,
            "« {state} » : jaquette de {cover} px au lieu de {expected}"
        );
    }

    // Contenus additionnels dans la colonne : les deux boutons s'empilent
    // au lieu de rétrécir le poids du jeu, et la carte ne monte pas.
    let (card, texts) = paint(
        &ctx,
        screen,
        340.0,
        SHEET_COMPACT_HEIGHT,
        CardButtons::Download {
            appid: 440,
            size: Some(10_100_000_000),
            dlc: 3,
            versions: Vec::new(),
            mobile: Vec::new(),
            chosen: 0,
            family: Vec::new(),
            family_chosen: 0,
            dlc_names: Vec::new(),
            steam: true,
        },
    );
    assert!(
        (card.height() - (COVER_BESIDE_HEIGHT + 20.0)).abs() < 1.0,
        "hauteur = {}",
        card.height()
    );
    let weight_label = lapinux_core::i18n::t_args(
        "game_sheet.download_size",
        &[("size", "10,1 Go")],
    );
    let weight = label_rect(&texts, &weight_label);
    let contents_rect = label_rect(&texts, "DLC · 3");
    assert!(
        contents_rect.top() > weight.bottom(),
        "les contenus ne s'empilent pas sous le poids"
    );
    for (text, rect) in &texts {
        assert!(
            card.contains_rect(*rect),
            "« {text} » sort de l'encart {card:?} / {rect:?}"
        );
    }

    // Versions publiées : la flèche prend sa largeur sur le bouton, sans
    // pousser le poids ni les contenus hors de l'encart.
    let (card, texts) = paint(
        &ctx,
        screen,
        340.0,
        SHEET_COMPACT_HEIGHT,
        CardButtons::Download {
            appid: 440,
            size: Some(10_100_000_000),
            dlc: 3,
            versions: vec![
                GameVersion {
                    id: "linux",
                    label: "Linux",
                    size: Some(9_500_000_000),
                },
                GameVersion {
                    id: "windows",
                    label: "Windows",
                    size: Some(10_100_000_000),
                },
            ],
            mobile: Vec::new(),
            chosen: 0,
            family: Vec::new(),
            family_chosen: 0,
            dlc_names: Vec::new(),
            steam: true,
        },
    );
    assert!(
        (card.height() - (COVER_BESIDE_HEIGHT + 20.0)).abs() < 1.0,
        "hauteur = {}",
        card.height()
    );
    assert!(
        labels(&texts).iter().any(|text| text == "DLC · 3"),
        "{:?}",
        labels(&texts)
    );
    for (text, rect) in &texts {
        assert!(
            card.contains_rect(*rect),
            "« {text} » sort de l'encart {card:?} / {rect:?}"
        );
    }

    // Sans contenu, le bouton occupe quand même tout le bloc : il ne se
    // rétrécit pas à son libellé, sinon la carte paraît bancale à côté
    // d'un jeu qui empile son DLC.
    let (card, texts) = paint(
        &ctx,
        screen,
        340.0,
        SHEET_COMPACT_HEIGHT,
        CardButtons::Download {
            appid: 440,
            size: Some(4_300_000_000),
            dlc: 0,
            versions: vec![
                GameVersion {
                    id: "linux",
                    label: "Linux",
                    size: Some(4_300_000_000),
                },
                GameVersion {
                    id: "windows",
                    label: "Windows",
                    size: Some(4_500_000_000),
                },
            ],
            mobile: Vec::new(),
            chosen: 0,
            family: Vec::new(),
            family_chosen: 0,
            dlc_names: Vec::new(),
            steam: true,
        },
    );
    let weight_label = lapinux_core::i18n::t_args(
        "game_sheet.download_size",
        &[("size", "4,3 Go")],
    );
    let weight = label_rect(&texts, &weight_label);
    let name_rect = label_rect(&texts, "8AM");
    // Le libellé est centré dans son bouton : le double de son écart au
    // bord de la colonne donne la largeur du bouton, flèche à part.
    let main = 2.0 * (weight.center().x - name_rect.left());
    let column = (340.0 - 20.0) - (name_rect.left() - card.left() - 20.0) - 10.0;
    let expected_block = column.min(CARD_ACTIONS_WIDTH);
    assert!(
        (main + VERSION_ARROW_WIDTH - expected_block).abs() < 2.0,
        "bloc de {} px au lieu de {expected_block}",
        main + VERSION_ARROW_WIDTH
    );

    // Le propriétaire familial : son avatar suit la flèche de version, dans
    // le même bloc, sans pousser le poids du jeu hors de l'encart.
    let (card, texts) = paint(
        &ctx,
        screen,
        340.0,
        SHEET_COMPACT_HEIGHT,
        CardButtons::Download {
            appid: 440,
            size: Some(10_100_000_000),
            dlc: 0,
            versions: vec![
                GameVersion {
                    id: "linux",
                    label: "Linux",
                    size: Some(9_500_000_000),
                },
                GameVersion {
                    id: "windows",
                    label: "Windows",
                    size: Some(10_100_000_000),
                },
            ],
            mobile: Vec::new(),
            chosen: 1,
            family: vec![FamilyVersion {
                steam_id: 2,
                name: "Alice".into(),
                avatar: String::new(),
                games: 2,
                texture: None,
                personal: false,
                dlc_names: Vec::new(),
            }],
            family_chosen: 0,
            dlc_names: Vec::new(),
            steam: true,
        },
    );
    let weight_label = lapinux_core::i18n::t_args(
        "game_sheet.download_size",
        &[("size", "10,1 Go")],
    );
    let weight = label_rect(&texts, &weight_label);
    let avatar = label_rect(&texts, "A");
    assert!(
        avatar.center().x > weight.right(),
        "l'avatar doit suivre le bouton de téléchargement : {avatar:?} / {weight:?}"
    );
    assert!(
        avatar.center().x - weight.right() < 80.0,
        "l'avatar reste collé au contrôle de version : {avatar:?} / {weight:?}"
    );
    for (text, rect) in &texts {
        assert!(
            card.contains_rect(*rect),
            "« {text} » sort de l'encart {card:?} / {rect:?}"
        );
    }
}

#[test]
fn family_versions_keep_only_members_who_share_the_game() {
    use lapinux_core::social::{SteamFamily, SteamFamilyMember};
    let family = SteamFamily {
        members: vec![
            SteamFamilyMember {
                steam_id: 1,
                is_self: true,
                appids: vec![440],
                ..Default::default()
            },
            SteamFamilyMember {
                steam_id: 2,
                name: "Alice".into(),
                games: 2,
                appids: vec![440, 570],
                ..Default::default()
            },
            SteamFamilyMember {
                steam_id: 3,
                games: 1,
                appids: vec![570],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let owners = family_versions_of(440, &family);
    assert_eq!(
        owners.len(),
        1,
        "le compte connecté et les autres jeux restent dehors"
    );
    assert_eq!(owners[0].steam_id, 2);
    assert_eq!(owners[0].name, "Alice");
    assert_eq!(owners[0].games, 2);
    // Un membre sans nom garde un libellé lisible, jamais un bouton vide.
    let unnamed = family_versions_of(570, &family);
    assert_eq!(unnamed.len(), 2);
    let member = unnamed
        .iter()
        .find(|member| member.steam_id == 3)
        .expect("membre sans nom");
    assert!(!member.name.trim().is_empty());
}

#[test]
fn family_versions_offer_my_version_first() {
    use lapinux_core::social::{SteamFamily, SteamFamilyMember};
    let family = SteamFamily {
        members: vec![
            SteamFamilyMember {
                steam_id: 1,
                is_self: true,
                appids: vec![440],
                ..Default::default()
            },
            SteamFamilyMember {
                steam_id: 2,
                name: "Alice".into(),
                games: 2,
                appids: vec![440],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let versions = family_versions_for(440, Some(&family), true);
    assert_eq!(versions.len(), 2);
    assert!(
        versions[0].personal,
        "« Moi » doit être la version par défaut"
    );
    assert_eq!(versions[0].steam_id, 1);
    assert!(!versions[1].personal);
    assert_eq!(versions[1].name, "Alice");
}

#[test]
fn a_late_family_snapshot_changes_the_library_key() {
    use lapinux_core::social::{SteamFamily, SteamFamilyMember, SteamSocial};
    let social = |appids: Vec<u32>| SteamSocial {
        family: Some(SteamFamily {
            members: vec![
                SteamFamilyMember {
                    steam_id: 1,
                    is_self: true,
                    appids: vec![440],
                    ..Default::default()
                },
                SteamFamilyMember {
                    steam_id: 2,
                    appids,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }),
        ..Default::default()
    };
    let one = crate::family_fingerprint(Some(&social(vec![550])));
    let two = crate::family_fingerprint(Some(&social(vec![550, 4000])));
    assert_ne!(one, two, "un jeu partagé en plus périme la liste");
    // L'ordre de la réponse ne doit pas périmer la liste sans raison.
    assert_eq!(
        two,
        crate::family_fingerprint(Some(&social(vec![4000, 550])))
    );
    assert_eq!(crate::family_fingerprint(None), 0);
}

#[test]
fn a_shared_owned_game_belongs_to_both_steam_sources() {
    let mut both = library_entry(7, "Both", false, 0, None, 0);
    both.family_available = true;
    let mut sources = SourceFilter::default();
    sources.family = false;
    assert!(sources.keeps(&both), "la source personnelle garde le jeu");
    sources.steam = false;
    sources.family = true;
    assert!(sources.keeps(&both), "la source famille garde aussi le jeu");
    sources.family = false;
    assert!(!sources.keeps(&both));
}

#[test]
fn the_download_menu_lists_linux_then_windows_then_macos() {
    // La première version est celle que la machine lance sans Proton :
    // Linux passe devant Windows, macOS ferme la liste.
    let details = SteamDetails {
        platforms: vec!["Windows".into(), "macOS".into(), "Linux".into()],
        size_linux: Some(lapinux_core::model::SteamSize {
            install: 200,
            download: 100,
        }),
        size_windows: Some(lapinux_core::model::SteamSize {
            install: 400,
            download: 300,
        }),
        ..Default::default()
    };
    let versions = game_versions(&details);
    assert_eq!(
        versions
            .iter()
            .map(|version| version.id)
            .collect::<Vec<_>>(),
        vec!["linux", "windows", "macos"]
    );
    assert_eq!(versions[0].size, Some(100), "poids du build Linux");
    assert_eq!(versions[1].size, Some(300), "poids du build Windows");
    // macOS est publié sans poids connu : il reste proposé, sans taille.
    assert_eq!(versions[2].size, None);
    // Un jeu Windows seul ne propose que Windows, la liste n'invente
    // jamais une plateforme que Steam ne publie pas.
    let windows_only = SteamDetails {
        platforms: vec!["Windows".into()],
        ..Default::default()
    };
    assert_eq!(game_versions(&windows_only).len(), 1);
    // Fiche pas encore relue : aucune version, le moteur garde la main.
    assert!(game_versions(&SteamDetails::default()).is_empty());
}

#[test]
fn the_version_menu_shows_the_published_versions_and_keeps_the_click() {
    // La flèche n'a de sens que si le menu s'ouvre vraiment, écrit le
    // poids de chaque version, et rend le choix au bouton.
    let ctx = egui::Context::default();
    apply_theme(&ctx);
    install_fonts(&ctx);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 420.0));
    let versions = vec![
        GameVersion {
            id: "linux",
            label: "Linux",
            size: Some(100),
        },
        GameVersion {
            id: "windows",
            label: "Windows",
            size: Some(300),
        },
    ];
    let mobile = vec![
        GameVersion {
            id: "android",
            label: "Android",
            size: None,
        },
        GameVersion {
            id: "ios",
            label: "iOS",
            size: None,
        },
    ];
    let mut action = CardAction::default();
    let mut labels: Vec<String> = Vec::new();
    // Ligne Windows : centre du libellé, où le clic tombe.
    let mut target = egui::pos2(10.0, 10.0);
    for frame in 0..4 {
        let button = |pressed: bool| egui::Event::PointerButton {
            pos: target,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let events = match frame {
            1 => vec![egui::Event::PointerMoved(target)],
            2 => vec![button(true)],
            3 => vec![button(false)],
            _ => Vec::new(),
        };
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let anchor = ui.label("ancre");
                    let id = egui::Id::new("version-menu-test");
                    ui.memory_mut(|memory| memory.open_popup(id));
                    version_menu(ui, id, &anchor, &versions, &mobile, 0, &mut action);
                });
            },
        );
        // Première image : l'Area du menu se mesure, ses textes
        // n'apparaissent qu'à la suivante.
        if frame != 1 {
            continue;
        }
        fn collect(shape: &egui::Shape, texts: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => texts.push((
                    text.galley.text().to_string(),
                    egui::Rect::from_min_size(text.pos, text.galley.rect.size()),
                )),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, texts);
                    }
                }
                _ => {}
            }
        }
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut texts);
        }
        labels = texts.iter().map(|(text, _)| text.clone()).collect();
        target = texts
            .iter()
            .find(|(text, _)| text.starts_with("Windows"))
            .map(|(_, rect)| rect.center())
            .unwrap_or(target);
    }
    assert!(
        labels.iter().any(|label| label == lapinux_core::i18n::t("game_sheet.download_version")),
        "le menu ne s'est pas ouvert : {labels:?}"
    );
    assert!(
        labels.iter().any(|label| label == "Linux · 100 o"),
        "{labels:?}"
    );
    assert!(
        labels.iter().any(|label| label == "Windows · 300 o"),
        "{labels:?}"
    );
    assert!(
        labels.iter().any(|label| label == lapinux_core::i18n::t("game_sheet.mobile_versions")),
        "{labels:?}"
    );
    assert!(labels.iter().any(|label| label == "Android"), "{labels:?}");
    assert!(labels.iter().any(|label| label == "iOS"), "{labels:?}");
    assert!(
        labels.iter().any(|label| label == lapinux_core::i18n::t("game_sheet.mobile_not_installable")),
        "{labels:?}"
    );
    // Le clic sur la ligne Windows est ce que le bouton recevra.
    assert_eq!(action.version, Some("windows"));
}

#[test]
fn the_chosen_version_is_the_one_the_download_asks_for() {
    let versions = vec![
        GameVersion {
            id: "linux",
            label: "Linux",
            size: Some(100),
        },
        GameVersion {
            id: "windows",
            label: "Windows",
            size: Some(300),
        },
    ];
    // Sans choix, la première version publiée part : Linux.
    assert_eq!(version_index(None, &versions), 0);
    // Un choix explicite l'emporte, c'est lui qui traversera l'IPC.
    assert_eq!(version_index(Some("windows"), &versions), 1);
    // Un choix qui n'est plus publié retombe sur la première version au
    // lieu de viser une case qui n'existe pas.
    assert_eq!(version_index(Some("macos"), &versions), 0);
    assert_eq!(version_index(Some("windows"), &[]), 0);
}

#[test]
fn the_download_weight_adds_the_selected_contents() {
    let dlc = vec![
        SteamDlc {
            appid: 1,
            name: "Un".into(),
            download: 100,
            install: 200,
            installed: false,
        },
        SteamDlc {
            appid: 2,
            name: "Deux".into(),
            download: 300,
            install: 400,
            installed: false,
        },
    ];
    // Rien de coché : le jeu de base seul.
    assert_eq!(download_size(Some(1000), &dlc, None), Some(1000));
    assert_eq!(
        download_size(Some(1000), &dlc, Some(&HashSet::new())),
        Some(1000)
    );
    // Un contenu coché allonge le poids annoncé.
    let selected: HashSet<u32> = [2].into_iter().collect();
    assert_eq!(download_size(Some(1000), &dlc, Some(&selected)), Some(1300));
    // Sans poids de base, ce sont les contenus qui comptent.
    assert_eq!(download_size(None, &dlc, Some(&selected)), Some(300));
    assert_eq!(download_size(None, &dlc, None), None);

    // Un contenu déjà sur le disque n'ajoute rien, même coché : il se
    // vérifie, il ne se recharge pas.
    let mut already = dlc.clone();
    already[1].installed = true;
    assert_eq!(
        download_size(Some(1000), &already, Some(&selected)),
        Some(1000)
    );
    assert_eq!(download_size(None, &already, Some(&selected)), None);
    // La sélection d'un contenu absent continue de compter.
    let both: HashSet<u32> = [1, 2].into_iter().collect();
    assert_eq!(download_size(Some(1000), &already, Some(&both)), Some(1100));
}

#[test]
fn the_sheet_slides_up_in_a_fraction_of_a_second() {
    // Départ fermé : la fiche monte progressivement.
    let mut progress = 0.0;
    let mut frames = 0;
    while progress < 1.0 {
        progress = advance_detail_sheet(progress, true, 1.0 / 60.0);
        frames += 1;
        assert!(frames < 120, "la fiche doit finir par monter");
    }
    assert_eq!(progress, 1.0);
    // Environ 0,18 s à 60 images par seconde.
    assert!((10..=12).contains(&frames), "montée en {frames} images");
    // Elle ne dépasse jamais ses bornes, et redescend de la même façon.
    assert_eq!(advance_detail_sheet(0.0, false, 1.0 / 60.0), 0.0);
    assert_eq!(advance_detail_sheet(1.0, true, 1.0 / 60.0), 1.0);
    assert!(advance_detail_sheet(0.2, false, 1.0 / 60.0) < 0.2);
    // Une image longue (démarrage, fenêtre en arrière-plan) ne fait pas
    // sauter la fiche d'un coup.
    assert!(advance_detail_sheet(0.0, true, 2.0) <= 0.56);
}

#[test]
fn the_side_columns_only_appear_on_a_wide_sheet() {
    // Largeur minimale de la fiche à trois colonnes : le centre y garde
    // juste sa largeur lisible et les côtés leur minimum.
    let threshold = SHEET_CENTER_MIN + SHEET_SIDE_MIN * 2.0 + SHEET_GAP * 2.0;
    // Fiche étroite : une seule colonne, les encarts s'empilent.
    assert_eq!(sheet_body_columns(700.0).0, 0.0);
    assert_eq!(sheet_body_columns(threshold - 1.0).0, 0.0);
    // Dès que le centre garde sa largeur lisible, les deux colonnes
    // latérales apparaissent, de largeur égale.
    let (left, center, right) = sheet_body_columns(threshold);
    assert!(left > 0.0 && (left - right).abs() < 0.5, "{left} / {right}");
    assert_eq!(left, SHEET_SIDE_MIN, "côtés à leur minimum");
    assert!((center - SHEET_CENTER_MIN).abs() < 0.5, "centre {center}");
    // Elles s'élargissent ensemble, sans jamais dépasser leur plafond.
    assert_eq!(sheet_body_columns(1_360.0).0, SHEET_SIDE_WIDTH);
    assert_eq!(sheet_body_columns(2_000.0).0, SHEET_SIDE_WIDTH);
}

/// La rangée du corps tient dans la fiche : les trois colonnes, les deux
/// écarts et l'espacement implicite d'egui ne dépassent pas la largeur
/// disponible. Sans la déduction de cet espacement, les images de droite
/// étaient coupées au bord de la fiche.
/// Les étiquettes de la page Protons sont des fonds pleins **sans
/// contour**, et les boutons portent leur ton : vert d'action, blanc de
/// vérification, gris de relecture.
#[test]
fn proton_tags_and_buttons_are_flat_and_toned() {
    fn paint(build: impl Fn(&mut egui::Ui)) -> (Vec<(egui::Color32, egui::Stroke)>, Vec<String>) {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut fills = Vec::new();
        let mut texts = Vec::new();
        for _ in 0..3 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 200.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, &build);
                },
            );
            fills.clear();
            texts.clear();
            fn walk(
                shape: &egui::Shape,
                fills: &mut Vec<(egui::Color32, egui::Stroke)>,
                texts: &mut Vec<String>,
            ) {
                match shape {
                    egui::Shape::Rect(rect) => fills.push((rect.fill, rect.stroke)),
                    egui::Shape::Text(text) => texts.push(text.galley.text().to_string()),
                    egui::Shape::Vec(shapes) => {
                        for shape in shapes {
                            walk(shape, fills, texts);
                        }
                    }
                    _ => {}
                }
            }
            for clipped in &output.shapes {
                walk(&clipped.shape, &mut fills, &mut texts);
            }
        }
        (fills, texts)
    }

    // Une étiquette : un aplat vert, aucun trait, le texte écrit dessus.
    let (fills, texts) = paint(|ui| {
        chip(ui, lapinux_core::i18n::t("protons.installed"), OK);
    });
    assert!(
        fills.iter().any(|(fill, _)| *fill == OK),
        "fond vert attendu : {fills:?}"
    );
    assert!(
        fills.iter().all(|(_, stroke)| stroke.width == 0.0),
        "aucun contour : {fills:?}"
    );
    assert!(
        texts
            .iter()
            .any(|text| text == lapinux_core::i18n::t("protons.installed")),
        "{texts:?}"
    );

    // Les quatre tons de bouton restent distincts et lisibles.
    for (tone, expected) in [
        (ActionTone::Primary, OK),
        (ActionTone::Light, ACCENT()),
        (ActionTone::Quiet, CARD_ACTIVE()),
        (ActionTone::Danger, mix(CARD(), DANGER, 0.14)),
    ] {
        let (fills, texts) = paint(|ui| {
            action_button(ui, lapinux_core::i18n::t("protons.install_all"), tone);
        });
        assert!(
            fills.iter().any(|(fill, _)| *fill == expected),
            "ton {tone:?} : {fills:?}"
        );
        assert!(
            texts
                .iter()
                .any(|text| text == lapinux_core::i18n::t("protons.install_all")),
            "libellé du bouton : {texts:?}"
        );
    }
    // Le survol et l'appui gardent le ton : seul l'éclat change.
    assert!(mix(OK, egui::Color32::WHITE, 1.0) == egui::Color32::WHITE);
    assert!(mix(OK, egui::Color32::WHITE, 0.0) == OK);
}

/// Les builds publiés : le dossier qu'une archive crée, la date écrite
/// courtement, le « latest » d'une source et la famille d'un build.
#[test]
fn proton_releases_are_read_by_folder_date_and_latest() {
    assert_eq!(
        release_folder("GE-Proton11-7-x86_64.tar.gz"),
        "GE-Proton11-7-x86_64"
    );
    assert_eq!(
        release_folder("proton-cachyos-11.0-20261015-slr-x86_64.tar.xz"),
        "proton-cachyos-11.0-20261015-slr-x86_64"
    );
    assert_eq!(release_folder("build.zip"), "build");

    assert_eq!(
        release_date("2026-10-01T12:00:00Z"),
        format!("1 {} 2026", lapinux_core::i18n::t("protons.month_10"))
    );
    assert_eq!(
        release_date("2025-01-09T02:03:04Z"),
        format!("9 {} 2025", lapinux_core::i18n::t("protons.month_1"))
    );
    assert_eq!(
        release_date(lapinux_core::i18n::t("workshop.soon")),
        lapinux_core::i18n::t("workshop.soon")
    );

    let release = |repo: &str, tag: &str, published: &str, prerelease: bool| ProtonRelease {
        repo: repo.to_string(),
        tag: tag.to_string(),
        asset: format!("{tag}-x86_64.tar.gz"),
        asset_url: String::new(),
        size: 1,
        checksum_url: None,
        published: published.to_string(),
        prerelease,
    };
    let ge = "GloriousEggroll/proton-ge-custom";
    let list = vec![
        release(ge, "GE-Proton11-6", "2026-09-01T00:00:00Z", false),
        release(ge, "GE-Proton11-7", "2026-10-01T00:00:00Z", false),
        // Une préversion plus récente ne doit pas passer devant la stable.
        release(ge, "GE-Proton11-8-rc1", "2026-10-20T00:00:00Z", true),
        release(
            "CachyOS/proton-cachyos",
            "cachy",
            "2026-10-30T00:00:00Z",
            false,
        ),
    ];
    let latest = latest_release(&list, ge).expect("un build pour GE");
    assert_eq!(latest.tag, "GE-Proton11-7");
    assert!(latest_release(
        &list,
        lapinux_core::i18n::t("controllers.information.unknown_band")
    )
    .is_none());

    // Famille : les noms installés restent reconnaissables même quand le
    // build affiché garde un nom commercial.
    assert_eq!(proton_family("GE-Proton11-7-x86_64"), "geproton");
    assert!(same_proton_family(
        "GE-Proton11-7-x86_64",
        "GE-Proton11-11-x86_64"
    ));
    assert!(same_proton_family(
        "Proton-CachyOS Latest",
        "proton-cachyos-11.0-20261015-slr-x86_64"
    ));
    assert!(same_proton_family(
        "DW-Proton Latest",
        "dwproton-11.0-14-x86_64"
    ));
    assert!(!same_proton_family("Proton 11.0", "GE-Proton11-11-x86_64"));

    assert_eq!(since_text(3), lapinux_core::i18n::t("downloads.just_now"));
    assert_eq!(
        since_text(120),
        lapinux_core::i18n::t_args("protons.since_minutes", &[("count", "2")])
    );
    assert_eq!(
        since_text(7_200),
        lapinux_core::i18n::t_args("protons.since_hours", &[("count", "2")])
    );
    assert_eq!(
        since_text(172_800),
        lapinux_core::i18n::t_args("protons.since_days", &[("count", "2")])
    );
}

/// Une ligne de source est une bande pleine largeur, haute de 64 px, dont
/// le clic n'importe où déplie les builds.
#[test]
fn a_proton_source_row_is_one_clickable_band() {
    let ctx = egui::Context::default();
    crate::install_fonts(&ctx);
    let mut heights = Vec::new();
    let mut clicked = Vec::new();
    for frame in 0..5 {
        let pos = egui::pos2(200.0, 20.0);
        let events = match frame {
            1 => vec![egui::Event::PointerMoved(pos)],
            2 => vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
            3 => vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
            _ => vec![],
        };
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 200.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let before = ui.cursor().min;
                    let hit = proton_source_header(
                        ui,
                        "GE-Proton",
                        "Proton communautaire avec les correctifs de Valve.",
                        &[(lapinux_core::i18n::t("protons.up_to_date"), OK)],
                        false,
                    );
                    heights.push(ui.cursor().min.y - before.y);
                    clicked.push(hit);
                });
            },
        );
        let _ = output;
    }
    // La bande fait 64 px ; l'espacement d'egui s'ajoute au curseur.
    assert!(
        heights.iter().all(|height| (height - 67.0).abs() < 0.5),
        "{heights:?}"
    );
    assert!(
        clicked.iter().any(|hit| *hit),
        "la bande doit répondre au clic : {clicked:?}"
    );
}

/// Une ligne de source déplie les builds de son dépôt, le plus récent en
/// tête, la préversion derrière la stable.
#[test]
fn proton_sources_list_their_builds_newest_first() {
    let release = |tag: &str, published: &str, prerelease: bool| ProtonRelease {
        repo: "GloriousEggroll/proton-ge-custom".to_string(),
        tag: tag.to_string(),
        asset: format!("{tag}-x86_64.tar.gz"),
        asset_url: String::new(),
        size: 1,
        checksum_url: None,
        published: published.to_string(),
        prerelease,
    };
    let list = vec![
        release("GE-Proton11-7", "2026-10-01T00:00:00Z", false),
        release("GE-Proton11-9-rc1", "2026-10-20T00:00:00Z", true),
        release("GE-Proton11-9", "2026-10-10T00:00:00Z", false),
        ProtonRelease {
            repo: "CachyOS/proton-cachyos".to_string(),
            tag: "cachy".to_string(),
            asset: "cachy-x86_64.tar.xz".to_string(),
            asset_url: String::new(),
            size: 1,
            checksum_url: None,
            published: "2026-10-30T00:00:00Z".to_string(),
            prerelease: false,
        },
    ];
    let builds = source_builds(&list, "GloriousEggroll/proton-ge-custom");
    let tags: Vec<&str> = builds.iter().map(|build| build.tag.as_str()).collect();
    assert_eq!(
        tags,
        vec!["GE-Proton11-9", "GE-Proton11-7", "GE-Proton11-9-rc1"],
        "la préversion passe après les stables"
    );
    assert!(source_builds(
        &list,
        lapinux_core::i18n::t("controllers.information.unknown_band")
    )
    .is_empty());
    // La ligne nomme le premier de la liste comme son « latest ».
    assert_eq!(
        builds.first().map(|build| build.tag.as_str()),
        Some("GE-Proton11-9")
    );
    // Chaque source a sa description : une ligne connue, un repli sinon.
    assert!(source_blurb("GloriousEggroll/proton-ge-custom").contains("codecs"));
    assert_eq!(
        source_blurb("inconnu/ailleurs"),
        lapinux_core::i18n::t("protons.blurb_fallback")
    );
}

/// Le sélecteur affiche le palier retenu : « Illimité » pour zéro, la
/// valeur suivie de son unité sinon.
#[test]
fn the_fps_picker_shows_the_selected_limit() {
    for (fps, expected) in [
        (0_u32, lapinux_core::i18n::t("downloads.unlimited")),
        (165, "165 FPS"),
    ] {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut value = fps;
        let mut seen: Vec<String> = Vec::new();
        for _ in 0..3 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 200.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        fps_picker(ui, "test-fps", &mut value);
                    });
                },
            );
            fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
                match shape {
                    egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
                    egui::Shape::Vec(shapes) => {
                        for shape in shapes {
                            walk(shape, out);
                        }
                    }
                    _ => {}
                }
            }
            seen.clear();
            for clipped in &output.shapes {
                walk(&clipped.shape, &mut seen);
            }
        }
        assert!(
            seen.iter().any(|text| text == expected),
            "« {expected} » attendu dans {seen:?}"
        );
        // Le sélecteur ne change pas la valeur : il ne fait que l'afficher.
        assert_eq!(value, fps);
    }
}

/// Le sélecteur de FPS : l'illimité en tête (« Illimité », donc zéro),
/// puis une montée régulière. Chaque palier doit survivre à la
/// normalisation de la configuration, sinon le réglage reviendrait à une
/// autre valeur au prochain enregistrement.
#[test]
fn the_fps_choices_start_with_unlimited_and_survive_normalization() {
    assert_eq!(FPS_CHOICES.first(), Some(&0));
    assert!(
        FPS_CHOICES.windows(2).all(|pair| pair[0] < pair[1]),
        "paliers croissants : {FPS_CHOICES:?}"
    );
    assert!(FPS_CHOICES.contains(&165), "un écran à 165 Hz s'y trouve");
    assert_eq!(fps_text(0), lapinux_core::i18n::t("downloads.unlimited"));
    assert_eq!(fps_text(165), "165 FPS");
    let mut config = lapinux_core::Config::default();
    for choice in FPS_CHOICES {
        config.launch.fps_cap = choice;
        assert_eq!(
            config.clone().normalized().launch.fps_cap,
            choice,
            "le palier {choice} change à la normalisation"
        );
    }
}

#[test]
fn the_sheet_body_row_stays_inside_the_sheet() {
    for width in [860.0, 1_000.0, 1_236.0, 1_500.0] {
        let (actus, center, media) = sheet_body_columns(width);
        assert!(actus > 0.0, "trois colonnes à {width}");
        let ctx = egui::Context::default();
        let mut spans: Vec<egui::Rect> = Vec::new();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 400.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.horizontal_top(|ui| {
                        let gap = sheet_column_gap(ui.spacing().item_spacing.x);
                        for (index, column) in [actus, center, media].into_iter().enumerate() {
                            if index > 0 {
                                ui.add_space(gap);
                            }
                            let drawn = ui.allocate_ui_with_layout(
                                egui::vec2(column, 0.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    ui.set_width(column);
                                    ui.allocate_exact_size(
                                        egui::vec2(column, 20.0),
                                        egui::Sense::hover(),
                                    );
                                },
                            );
                            spans.push(drawn.response.rect);
                        }
                    });
                });
            },
        );
        let span = spans.last().unwrap().right() - spans[0].left();
        assert!(
            span <= width + 0.5,
            "la rangée déborde à {width} : {span} pour {width}"
        );
        assert!(
            (span - width).abs() < 2.0,
            "la rangée ne remplit pas la fiche à {width} : {span}"
        );
    }
}

/// Les marques des boutiques sont des silhouettes blanches embarquées :
/// elles doivent se décoder, rester blanches et couvrir une part lisible de
/// leur boîte — un fichier vide ne laisserait qu'un carré dans la liste.
#[test]
fn the_store_marks_decode_as_white_silhouettes() {
    for store in [
        lapinux_core::stores::Store::Epic,
        lapinux_core::stores::Store::Gog,
        lapinux_core::stores::Store::Itch,
    ] {
        let image = store_mark_image(store_mark_bytes(store), 128);
        let total = (image.size[0] * image.size[1]) as f32;
        let mut painted = 0.0;
        for pixel in &image.pixels {
            // egui prémultiplie en linéaire : seule la neutralité de la
            // teinte et le blanc des pixels pleins se lisent simplement.
            assert!(
                pixel.r() == pixel.g() && pixel.g() == pixel.b(),
                "{} se dessine en blanc",
                store.label()
            );
            if pixel.a() >= 250 {
                assert!(
                    pixel.r() >= 250,
                    "{} se dessine en blanc (r = {})",
                    store.label(),
                    pixel.r()
                );
            }
            if pixel.a() > 128 {
                painted += 1.0;
            }
        }
        let share = painted / total;
        assert!(
            (0.10..0.95).contains(&share),
            "{} couvre {:.0} % de sa boîte",
            store.label(),
            share * 100.0
        );
    }
}

/// Le logo fourni est un disque noir au glyphe blanc : posé tel quel sur une
/// carte sombre, seul le disque se confondait avec le fond. Recomposé, la
/// marque doit tenir à la taille d'un pictogramme — glyphe plein, disque
/// léger mais présent — et être posée en image.
#[test]
fn the_steam_mark_reads_on_a_dark_background() {
    let image = steam_mark_image(128);
    let [width, height] = image.size;
    let alpha = |x: f32, y: f32| {
        let column = (x * width as f32) as usize;
        let row = (y * height as f32) as usize;
        image.pixels[row.min(height - 1) * width + column.min(width - 1)].a() as f32
    };
    // Le centre du glyphe, un point du disque loin du glyphe, puis un coin
    // transparent dans le fichier.
    assert!(alpha(0.5, 0.5) > 240.0, "le glyphe reste plein");
    let disc = alpha(0.5, 0.06);
    assert!(disc > 20.0, "le disque se détache du fond (alpha = {disc})");
    assert!(disc < 60.0, "le disque reste discret (alpha = {disc})");
    assert_eq!(alpha(0.02, 0.02), 0.0, "le coin reste transparent");

    fn walk(shape: &egui::Shape, marks: &mut Vec<(egui::TextureId, egui::Rect)>) {
        match shape {
            egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                let rect = mesh
                    .vertices
                    .iter()
                    .fold(egui::Rect::NOTHING, |rect, vertex| {
                        rect.union(egui::Rect::from_min_max(vertex.pos, vertex.pos))
                    });
                marks.push((mesh.texture_id, rect));
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, marks);
                }
            }
            _ => {}
        }
    }

    let ctx = egui::Context::default();
    apply_theme(&ctx);
    let expected = steam_mark_texture(&ctx, mark_pixels(&ctx, 11.4)).id();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            paint_steam_icon(ui.painter(), egui::pos2(60.0, 40.0), TEXT(), 1.0);
        });
    });
    let mut marks = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut marks);
    }
    assert_eq!(marks.len(), 1, "une seule marque peinte");
    let (texture, rect) = marks[0];
    assert_eq!(texture, expected, "c'est bien le logo embarqué");
    assert!(
        (rect.width() - 11.4).abs() < 0.5 && (rect.height() - 11.4).abs() < 0.5,
        "la marque occupe la place de l'ancien tracé ({} × {})",
        rect.width(),
        rect.height()
    );
    assert!(
        (rect.center() - egui::pos2(60.0, 40.0)).length() < 0.5,
        "la marque est centrée sur le point demandé"
    );
}

#[test]
fn token_age_reads_in_minutes_hours_then_days() {
    assert_eq!(token_age(0), "enregistré à l'instant");
    assert_eq!(token_age(-40), "enregistré à l'instant");
    assert_eq!(token_age(90), "enregistré il y a 1 min");
    assert_eq!(token_age(7_500), "enregistré il y a 2 h 05");
    assert_eq!(token_age(86_400), "enregistré il y a 1 j");
    assert_eq!(token_age(86_400 * 9 + 500), "enregistré il y a 9 j");
}

/// Le bandeau de faits sous les succès : âge, développeur, éditeur, prix,
/// année. Rien d'inventé quand Steam n'a rien publié.
#[test]
fn the_sheet_facts_line_reads_age_people_price_and_year() {
    let details = SteamDetails {
        developers: vec!["IO Interactive".into()],
        genres: Vec::new(),
        publishers: vec!["IO Interactive".into()],
        release_date: Some(1_780_000_000),
        ..Default::default()
    };
    let info = SteamGameInfo {
        update: lapinux_core::model::SteamUpdateInfo {
            ratings: vec!["PEGI 18".into()],
            ..Default::default()
        },
        store: lapinux_core::model::SteamStorePage {
            price: "59,99 €".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let facts = sheet_facts(&details, Some(&info), None);
    assert_eq!(
        facts.first().map(|(label, _)| *label),
        Some(lapinux_core::i18n::t("game_sheet.fact_age"))
    );
    assert_eq!(
        facts.last().map(|(label, _)| *label),
        Some(lapinux_core::i18n::t("game_sheet.fact_release"))
    );
    let joined = facts
        .iter()
        .map(|(label, value)| format!("{label} {value}"))
        .collect::<Vec<_>>()
        .join(" · ");
    assert!(joined.contains("PEGI 18"), "{joined}");
    assert!(joined.contains("IO Interactive"), "{joined}");
    assert!(joined.contains("59,99 €"), "{joined}");
    assert!(joined.ends_with("2026"), "{joined}");
    // L'éditeur ne se répète pas quand il est aussi le développeur.
    assert_eq!(
        facts
            .iter()
            .filter(|(l, _)| *l == lapinux_core::i18n::t("collections.publisher"))
            .count(),
        0
    );
    assert_eq!(joined.matches("IO Interactive").count(), 1, "{joined}");
    // Sans aucune source, aucune bande de tags.
    assert!(sheet_facts(&SteamDetails::default(), None, None).is_empty());
}

/// Les colonnes du corps tiennent dans la fiche : annonces à gauche,
/// description au centre, images à droite ; une seule colonne quand la
/// fiche est trop étroite.
#[test]
fn la_fiche_propose_les_six_onglets_quand_tout_est_disponible() {
    let tabs = sheet_tabs(true, true, true);
    assert_eq!(tabs.len(), 6);
    assert!(tabs.iter().any(|(tab, _, _)| *tab == SheetTab::SteamInput));
    assert!(tabs.iter().any(|(tab, _, _)| *tab == SheetTab::Workshop));
}

#[test]
fn the_sheet_body_columns_fit_the_sheet() {
    for width in [700.0, 860.0, 1_000.0, 1_080.0, 1_236.0, 1_900.0] {
        let (actus, center, media) = sheet_body_columns(width);
        if actus <= 0.0 {
            assert_eq!(center, width, "une seule colonne à {width}");
            assert_eq!(media, 0.0, "pas d'images empilées à {width}");
            continue;
        }
        let total = actus + center + media + SHEET_GAP * 2.0;
        assert!(
            (total - width).abs() < 0.5,
            "les colonnes ne remplissent pas la fiche à {width} : {total}"
        );
        assert!(
            center >= SHEET_CENTER_MIN,
            "le centre reste lisible à {width}"
        );
        // Les deux colonnes latérales sont égales : la description du jeu
        // est donc exactement au milieu de la fiche.
        assert!(
            (actus - media).abs() < 0.5,
            "colonnes inégales à {width} : {actus} / {media}"
        );
        assert!(
            ((width - center) / 2.0 - (actus + SHEET_GAP)).abs() < 0.5,
            "description décentrée à {width}"
        );
    }
}

/// Compte les traits d'une forme, en descendant les groupes.
fn count_strokes(shape: &egui::Shape, counter: &mut usize) {
    match shape {
        egui::Shape::LineSegment { .. } => *counter += 1,
        egui::Shape::Path(path) => *counter += usize::from(!path.points.is_empty()),
        egui::Shape::Circle(circle) => {
            if circle.fill != egui::Color32::TRANSPARENT {
                *counter += 1;
            }
        }
        egui::Shape::Vec(shapes) => {
            for inner in shapes {
                count_strokes(inner, counter);
            }
        }
        _ => {}
    }
}

/// Reproduction : le curseur doit se poser sous la souris, pas au début.
#[test]
fn the_cursor_follows_the_pointer() {
    let ctx = egui::Context::default();
    apply_theme(&ctx);
    let mut raw = egui::RawInput::default();
    raw.screen_rect = Some(egui::Rect::from_min_size(
        egui::pos2(0.0, 0.0),
        egui::vec2(1200.0, 800.0),
    ));
    let series: Vec<u32> = (0..120).map(|index| 60 + (index % 5) as u32).collect();
    // Première image : la mise en page. La seconde reçoit la souris.
    let mut first = raw.clone();
    first.events.clear();
    let _ = ctx.run(first, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(100.0);
            ui.horizontal(|ui| {
                ui.add_space(100.0);
                fps_sparkline(ui, &series, 1_700_000_000, 1_700_000_600, 600.0);
            });
        });
    });
    raw.events
        .push(egui::Event::PointerMoved(egui::pos2(600.0, 130.0)));
    let output = ctx.run(raw, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(100.0);
            ui.horizontal(|ui| {
                ui.add_space(100.0);
                fps_sparkline(ui, &series, 1_700_000_000, 1_700_000_600, 600.0);
            });
        });
    });
    let mut marker = None;
    for clipped in &output.shapes {
        if let egui::Shape::LineSegment { points, .. } = &clipped.shape {
            let (a, b) = (points[0], points[1]);
            if (a.x - b.x).abs() < 0.1 && (a.y - b.y).abs() > 20.0 {
                marker = Some(a.x);
            }
        }
    }
    let marker = marker.expect("curseur absent : le graphique n'a pas été survolé");
    // Graphique posé à x = 100 (marge) + 8 (bord intérieur).
    let left = 108.0;
    let expected = left + (600.0 - left);
    assert!(
        (marker - 600.0).abs() < 6.0,
        "curseur à {marker} au lieu de {expected}"
    );
}

#[test]
fn fps_curve_draws_its_strokes() {
    let ctx = egui::Context::default();
    apply_theme(&ctx);
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            fps_sparkline(ui, &[45, 60, 120, 90, 144, 130], 0, 0, 520.0);
        });
    });
    let mut strokes = 0;
    for clipped in &output.shapes {
        count_strokes(&clipped.shape, &mut strokes);
    }
    assert!(strokes >= 2, "courbe et point final absents : {strokes}");
}

/// La couleur d'un point suit la cadence : vert quand la session tient son
/// maximum, jaune quand elle fléchit, rouge sur une chute.

/// Les parties et les téléchargements d'un même jour se rangent dans la
/// même catégorie, la journée la plus récente en tête, et ce qui tourne
/// encore passe devant.
#[test]
fn a_day_gathers_sessions_and_downloads() {
    let now = 1_700_000_000;
    let today = lapinux_core::util::day_start(now);
    let card = |name: &str, started: i64, ended: Option<i64>| ActivityCard {
        appid: 0,
        name: name.into(),
        store: None,
        remote: String::new(),
        detail: String::new(),
        tag: String::new(),
        tone: TEXT(),
        progress: None,
        started: Some(started),
        ended,
        log: String::new(),
        fps: None,
        fps_series: Vec::new(),
        fps_series_start: 0,
        fps_series_end: 0,
        delete: None,
    };
    let cards = vec![
        card("hier", today - 86_400, Some(today - 86_400 + 600)),
        card("ce matin", today + 600, Some(today + 1200)),
        card(
            lapinux_core::i18n::t("downloads.download_lower"),
            today + 3_600,
            Some(today + 4_000),
        ),
        card(lapinux_core::i18n::t("saves.in_game"), today + 5_000, None),
    ];
    let days = group_by_day(cards, now);
    assert_eq!(days.len(), 2, "un jour par catégorie");
    assert_eq!(days[0].0, today);
    let names: Vec<&str> = days[0].1.iter().map(|card| card.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            lapinux_core::i18n::t("saves.in_game"),
            lapinux_core::i18n::t("downloads.download_lower"),
            "ce matin"
        ],
        "le jeu en cours d'abord, puis du plus récent au plus ancien"
    );
    assert_eq!(days[1].0, today - 86_400);
    assert_eq!(days[1].1.len(), 1);
}

/// Une carte sans horodatage tombe dans la journée courante, pas en 1970.
#[test]
fn an_undated_card_lands_today() {
    let now = 1_700_000_000;
    let today = lapinux_core::util::day_start(now);
    let card = ActivityCard {
        appid: 0,
        name: "sans date".into(),
        store: None,
        remote: String::new(),
        detail: String::new(),
        tag: String::new(),
        tone: TEXT(),
        progress: None,
        started: None,
        ended: None,
        log: String::new(),
        fps: None,
        fps_series: Vec::new(),
        fps_series_start: 0,
        fps_series_end: 0,
        delete: None,
    };
    let days = group_by_day(vec![card], now);
    assert_eq!(days.len(), 1);
    assert_eq!(days[0].0, today);
}

#[test]
fn cover_requests_have_their_own_budget() {
    let mut covers = CoverCache::new("", false);
    covers.begin_frame(6);
    assert_eq!(covers.decode_budget, 6);
    // Les lectures locales peuvent consommer tout le décodage : la demande
    // réseau garde son propre créneau, sinon plus aucune jaquette ne part.
    assert_eq!(covers.request_budget, 3);
    assert!(covers.take_request());
    assert!(covers.take_request());
    assert!(covers.take_request());
    assert!(!covers.take_request());
    // Un défilement rapide ferme aussi les demandes.
    covers.begin_frame(0);
    assert_eq!(covers.decode_budget, 0);
    assert_eq!(covers.request_budget, 0);
    assert!(!covers.take_request());
}

#[test]
fn les_jaquettes_absentes_ne_bloquent_pas_les_suivantes_au_demarrage() {
    let dir = tempfile::tempdir().unwrap();
    let paths = LapinuxPaths::under(dir.path());
    std::fs::create_dir_all(paths.covers_dir()).unwrap();
    let png = banner_png(20, 30);
    for appid in 100..108 {
        std::fs::write(paths.covers_dir().join(format!("{appid}.jpg")), &png).unwrap();
    }
    let ctx = egui::Context::default();
    let mut covers = CoverCache::new("", false);
    covers.begin_frame(6);
    for appid in 1..=6 {
        covers.missing.insert((appid, CoverTier::Tile));
        covers.steam_cache.insert(appid, None);
        assert!(covers
            .get(&ctx, &paths, appid, "absente", None, "", CoverTier::Tile)
            .is_none());
    }
    assert_eq!(
        covers.decode_budget, 6,
        "les fichiers absents ne se décodent pas"
    );
    for appid in 100..106 {
        assert!(covers
            .get(&ctx, &paths, appid, "jeu", None, "", CoverTier::Tile)
            .is_some());
    }
    assert!(covers
        .get(&ctx, &paths, 106, "jeu", None, "", CoverTier::Tile)
        .is_none());
    covers.begin_frame(6);
    for appid in 106..108 {
        assert!(covers
            .get(&ctx, &paths, appid, "jeu", None, "", CoverTier::Tile)
            .is_some());
    }
}

#[test]
fn une_jaquette_de_boutique_devient_l_icone_du_survol() {
    let dir = tempfile::tempdir().unwrap();
    let paths = LapinuxPaths::under(dir.path());
    paths.ensure().unwrap();
    std::fs::create_dir_all(paths.covers_dir()).unwrap();
    let cover = paths.covers_dir().join("store-42.jpg");
    std::fs::write(&cover, banner_png(20, 20)).unwrap();
    assert_eq!(cover_for_icon(&paths, 42), Some(cover));
    let ctx = egui::Context::default();
    assert!(square_icon_from_cover(&ctx, &paths, 42).is_some());
}

#[test]
fn une_jaquette_en_attente_de_decodage_redemande_une_image() {
    let dir = tempfile::tempdir().unwrap();
    let paths = LapinuxPaths::under(dir.path());
    std::fs::create_dir_all(paths.covers_dir()).unwrap();
    std::fs::write(paths.covers_dir().join("store-42.jpg"), banner_png(20, 30)).unwrap();
    let ctx = egui::Context::default();
    let mut covers = CoverCache::new("", false);
    covers.begin_frame(1);
    assert!(covers.take_decode());
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        assert!(covers
            .get(
                ctx,
                &paths,
                42,
                "jeu",
                Some(lapinux_core::stores::Store::Gog),
                "",
                CoverTier::Tile,
            )
            .is_none());
    });
    assert!(
        output.viewport_output[&egui::ViewportId::ROOT].repaint_delay <= Duration::from_millis(50)
    );
    covers.begin_frame(1);
    assert!(covers
        .get(
            &ctx,
            &paths,
            42,
            "jeu",
            Some(lapinux_core::stores::Store::Gog),
            "",
            CoverTier::Tile,
        )
        .is_some());
}

#[test]
fn les_demandes_reseau_en_attente_redemandent_une_image() {
    let dir = tempfile::tempdir().unwrap();
    let paths = LapinuxPaths::under(dir.path());
    let ctx = egui::Context::default();
    let mut covers = CoverCache::new("", false);
    covers.steam_cache.insert(42, None);
    covers.begin_frame(1);
    assert!(covers.take_request());
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        assert!(covers
            .get(ctx, &paths, 42, "jeu", None, "", CoverTier::Tile)
            .is_none());
    });
    assert!(
        output.viewport_output[&egui::ViewportId::ROOT].repaint_delay <= Duration::from_millis(50)
    );
    assert!(!covers.requested.contains_key(&(42, CoverTier::Tile)));
}

#[test]
fn first_rows_can_lift_the_request_budget() {
    let mut covers = CoverCache::new("", false);
    covers.begin_frame(1);
    assert_eq!(covers.request_budget, 1);
    covers.boost_requests(12);
    assert_eq!(covers.request_budget, 12);
    // Le préchargement ne donne pas de crédits de décodage en plus : les
    // textures lourdes restent réparties sur les images suivantes.
    assert_eq!(covers.decode_budget, 1);
}

#[test]
fn first_rows_move_to_the_front_of_store_prefetch() {
    use lapinux_core::stores::Store;

    let mut covers = CoverCache::new("", false);
    for appid in [1, 2, 3] {
        covers
            .prefetch
            .push_back((appid, String::new(), Store::Gog));
    }
    covers.prioritize_prefetch(&[3, 1]);
    let order: Vec<u32> = covers.prefetch.iter().map(|(appid, _, _)| *appid).collect();
    // L'ordre de la file est conservé entre les jeux prioritaires.
    assert_eq!(order, vec![1, 3, 2]);
}

#[test]
fn hover_origin_keeps_the_store_name() {
    use lapinux_core::stores::Store;

    let epic = game_origin_label(Some(Store::Epic), false, false, false);
    assert!(epic.contains("Epic Games"), "origine : {epic}");
    assert!(!epic.contains("{}"), "origine : {epic}");

    let steam = game_origin_label(None, true, false, false);
    assert!(steam.contains("Steam"), "origine : {steam}");

    assert_eq!(
        game_store_mark(Some(Store::Gog), false, false, false),
        Some(Store::Gog)
    );
    assert_eq!(
        game_store_mark(None, true, false, false),
        Some(Store::Steam)
    );
    assert_eq!(game_store_mark(None, false, false, false), None);
}

#[test]
fn the_curve_turns_yellow_then_red() {
    assert_eq!(fps_tone(160, 165), OK);
    // 60 et 120 images restent vertes : le maximum observé d'une session ne
    // doit pas transformer une cadence confortable en alerte.
    assert_eq!(fps_tone(120, 165), OK);
    assert_eq!(fps_tone(60, 165), OK);
    assert_eq!(fps_tone(50, 165), WARN);
    assert_eq!(fps_tone(40, 165), DANGER);
    // Une session lente mais régulière reste verte : c'est la chute qui
    // se signale, pas la cadence absolue.
    assert_eq!(fps_tone(30, 30), OK);
}

/// La courbe se place sur la même bande que la jaquette, à droite de la
/// colonne du titre — pas sous la carte.
#[test]
fn activity_card_aligns_the_fps_curve_beside_the_cover() {
    let ctx = egui::Context::default();
    apply_theme(&ctx);
    let paths = LapinuxPaths::under(std::path::Path::new("/lapinux-aucun"));
    let mut covers = CoverCache::new("", false);
    let card = ActivityCard {
        appid: 0,
        name: "MOTORSLICE".into(),
        store: None,
        remote: String::new(),
        detail: "Lancée à 21:38 · fermée à 21:39".into(),
        tag: "1 min".into(),
        tone: TEXT_DIM(),
        progress: None,
        started: None,
        ended: None,
        log: String::new(),
        fps: Some(lapinux_core::model::FpsStats {
            current: 150,
            min: 149,
            avg: 163,
            max: 165,
        }),
        fps_series: vec![150, 160, 155, 165, 158, 162],
        fps_series_start: 0,
        fps_series_end: 0,
        delete: None,
    };
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.set_width(900.0);
            let _ = activity_card(ui, &mut covers, &paths, &card);
        });
    });
    let mut card_rect = None;
    let mut curve_rect = None;
    for clipped in &output.shapes {
        if let egui::Shape::Rect(rect) = &clipped.shape {
            if rect.fill == CARD() && rect.rect.width() > 500.0 {
                card_rect = Some(rect.rect);
            }
            if rect.fill == PANEL() && rect.rect.width() > 100.0 {
                curve_rect = Some(rect.rect);
            }
        }
    }
    let card_rect = card_rect.expect("carte absente");
    let curve_rect = curve_rect.expect("courbe absente");
    // Marge intérieure de la carte, la jaquette (46 px) et son écart : le
    // bloc du titre commence là, et la courbe occupe toute sa largeur.
    let expected = card_rect.left() + 10.0 + 46.0 + 12.0;
    let gap = (curve_rect.left() - expected).abs();
    assert!(
        gap < 1.5,
        "courbe décalée : {} au lieu de {expected}",
        curve_rect.left()
    );
}

#[test]
fn a_single_measurement_has_no_curve() {
    let ctx = egui::Context::default();
    apply_theme(&ctx);
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            fps_sparkline(ui, &[60], 0, 0, 520.0);
        });
    });
    let mut strokes = 0;
    for clipped in &output.shapes {
        count_strokes(&clipped.shape, &mut strokes);
    }
    assert_eq!(strokes, 0, "un seul point ne fait pas un graphique");
}

/// Fabrique une entrée de bibliothèque minimale pour les tests de vue.
fn library_entry(
    appid: u32,
    name: &str,
    installed: bool,
    playtime: u64,
    size: Option<u64>,
    last_played: i64,
) -> LibraryEntry {
    LibraryEntry {
        appid,
        name: name.to_string(),
        installed,
        on_disk: installed,
        steam: true,
        family: false,
        family_available: false,
        update: None,
        store: None,
        cover: String::new(),
        platforms: Vec::new(),
        mobile: false,
        size,
        publishers: vec!["Ubisoft".to_string()],
        developers: vec!["Ancel Studio".to_string()],
        genres: Vec::new(),
        playtime,
        acquired: None,
        updated: 0,
        last_played,
    }
}

/// La recherche replie accents et casse : « pokemon » trouve « Pokémon ».
#[test]
fn search_folds_accents_and_case() {
    assert_eq!(fold_search("Pokémon"), "pokemon");
    assert_eq!(fold_search("ÉDITEUR"), "editeur");
    assert_eq!(fold_search("Cœur"), "coeur");
    let tokens = search_tokens("POKEMON editeur");
    assert_eq!(tokens, vec!["pokemon".to_string(), "editeur".to_string()]);
    assert!(matches_tokens("pokemon editeur", &tokens));
    assert!(!matches_tokens("pokemon", &tokens));
}

/// La recherche porte aussi sur les crédits, pas seulement sur le titre.
#[test]
fn search_matches_publisher_and_developer() {
    let entry = library_entry(1, "Rayman", true, 0, None, 0);
    let text = entry_search_text(&entry);
    assert!(matches_tokens(&text, &search_tokens("ubisoft")));
    assert!(matches_tokens(&text, &search_tokens("ancel")));
    assert!(matches_tokens(&text, &search_tokens("ubisoft rayman")));
    assert!(!matches_tokens(&text, &search_tokens("nintendo")));
}

/// Un jeu installé ne passe plus devant un jeu du catalogue : seul le tri
/// choisi décide de l'ordre.
#[test]
fn installed_games_follow_the_sort() {
    let installed = library_entry(1, "Zeta", true, 10, None, 0);
    let missing = library_entry(2, "Alpha", false, 99, None, 0);
    // Par temps de jeu décroissant, le jeu non installé passe devant.
    assert_eq!(
        compare_library(&installed, &missing, LibrarySort::Playtime, true),
        std::cmp::Ordering::Greater
    );
    // Par nom croissant, c'est le titre qui décide, installation ou non.
    assert_eq!(
        compare_library(&missing, &installed, LibrarySort::Name, false),
        std::cmp::Ordering::Less
    );
}

/// Le sens s'inverse pour tous les tris, et le nom tranche les égalités.
#[test]
fn sort_direction_reverses_the_primary_key() {
    let short = library_entry(1, "Aaa", true, 30, None, 0);
    let long = library_entry(2, "Bbb", true, 300, None, 0);
    assert_eq!(
        compare_library(&short, &long, LibrarySort::Playtime, false),
        std::cmp::Ordering::Less
    );
    assert_eq!(
        compare_library(&short, &long, LibrarySort::Playtime, true),
        std::cmp::Ordering::Greater
    );
    let same = library_entry(3, "Aaa", true, 30, None, 0);
    assert_eq!(
        compare_library(&short, &same, LibrarySort::Playtime, true),
        std::cmp::Ordering::Equal
    );
}

/// Le tri par taille lit le poids publié par la boutique, et range les
/// jeux sans poids en dernier quand l'ordre est décroissant.
#[test]
fn size_sort_uses_the_published_weight() {
    let big = library_entry(1, "Big", true, 0, Some(80_000_000_000), 0);
    let small = library_entry(2, "Small", true, 0, Some(1_000_000), 0);
    let unknown = library_entry(3, "Unknown", true, 0, None, 0);
    assert_eq!(
        compare_library(&big, &small, LibrarySort::Size, true),
        std::cmp::Ordering::Less
    );
    assert_eq!(
        compare_library(&unknown, &small, LibrarySort::Size, true),
        std::cmp::Ordering::Greater
    );
}

/// La date d'obtention part du plus récent, et les fiches que Steam ne
/// date pas restent en fin de liste.
#[test]
fn acquired_sort_starts_newest_and_keeps_undated_last() {
    let mut recent = library_entry(1, "Recent", true, 0, None, 0);
    recent.acquired = Some(1_800_000_000);
    let mut older = library_entry(2, "Older", true, 0, None, 0);
    older.acquired = Some(1_700_000_000);
    let undated = library_entry(3, "Undated", true, 0, None, 0);
    assert!(LibrarySort::Acquired.default_descending());
    assert_eq!(
        compare_library(
            &recent,
            &older,
            LibrarySort::Acquired,
            LibrarySort::Acquired.default_descending()
        ),
        std::cmp::Ordering::Less
    );
    assert_eq!(
        compare_library(&undated, &older, LibrarySort::Acquired, true),
        std::cmp::Ordering::Greater
    );
}

/// Le tri « récemment joués » lit la date de la dernière partie.
#[test]
fn recent_sort_reads_the_last_session_date() {
    let old = library_entry(1, "Old", true, 0, None, 1_700_000_000);
    let new = library_entry(2, "New", true, 0, None, 1_800_000_000);
    assert_eq!(
        compare_library(&old, &new, LibrarySort::Recent, true),
        std::cmp::Ordering::Greater
    );
}

/// Un snapshot de reconnexion ou une pause du catalogue ne doit pas retirer
/// le propriétaire familial déjà affiché sur la fiche.
#[test]
fn a_social_snapshot_without_family_keeps_the_last_owner() {
    let previous = lapinux_core::social::SteamSocial {
        family: Some(family_with_one_shared_game()),
        ..lapinux_core::social::SteamSocial::default()
    };
    let mut paused = lapinux_core::social::SteamSocial::connecting();
    keep_family_through_snapshot(Some(&previous), &mut paused, false);
    assert!(paused.family.is_some());
}

/// À l'inverse, un état vivant sans famille dit que le groupe a réellement
/// disparu : le bouton doit s'effacer.
#[test]
fn a_live_snapshot_without_family_removes_the_owner() {
    let previous = lapinux_core::social::SteamSocial {
        family: Some(family_with_one_shared_game()),
        ..lapinux_core::social::SteamSocial::default()
    };
    let mut live = lapinux_core::social::SteamSocial {
        state: "live".into(),
        ..lapinux_core::social::SteamSocial::default()
    };
    keep_family_through_snapshot(Some(&previous), &mut live, false);
    assert!(live.family.is_none());
}

/// Pendant une reconnexion, Steam renvoie les membres avant leurs appids et
/// leurs identités : le snapshot doit conserver ces détails.
#[test]
fn a_partial_family_snapshot_keeps_appids_names_and_avatars() {
    let previous = lapinux_core::social::SteamSocial {
        family: Some(family_with_one_shared_game()),
        ..lapinux_core::social::SteamSocial::default()
    };
    let mut live = lapinux_core::social::SteamSocial {
        state: "live".into(),
        family: Some(lapinux_core::social::SteamFamily {
            group_id: 7,
            name: "Maison".into(),
            members: vec![lapinux_core::social::SteamFamilyMember {
                steam_id: 42,
                ..Default::default()
            }],
        }),
        ..lapinux_core::social::SteamSocial::default()
    };
    keep_family_through_snapshot(Some(&previous), &mut live, false);
    let member = &live.family.as_ref().unwrap().members[0];
    assert_eq!(member.appids, vec![3712150]);
    assert_eq!(member.name, "Yuri");
    assert_eq!(member.avatar, "https://example.invalid/avatar.jpg");
    assert_eq!(member.games, 334);
}

/// Une reconnexion peut publier un état vivant avant d'avoir relu le groupe :
/// la famille vue récemment reste affichée le temps de cette relecture.
#[test]
fn a_live_snapshot_during_reconnection_keeps_a_recent_family() {
    let previous = lapinux_core::social::SteamSocial {
        family: Some(family_with_one_shared_game()),
        ..lapinux_core::social::SteamSocial::default()
    };
    let mut live = lapinux_core::social::SteamSocial {
        state: "live".into(),
        ..lapinux_core::social::SteamSocial::default()
    };
    keep_family_through_snapshot(Some(&previous), &mut live, true);
    assert!(live.family.is_some());
}

fn family_with_one_shared_game() -> lapinux_core::social::SteamFamily {
    lapinux_core::social::SteamFamily {
        group_id: 7,
        name: "Maison".into(),
        members: vec![lapinux_core::social::SteamFamilyMember {
            steam_id: 42,
            name: "Yuri".into(),
            role: 1,
            is_self: false,
            avatar: "https://example.invalid/avatar.jpg".into(),
            games: 334,
            appids: vec![3712150],
        }],
    }
}

/// L'icône communautaire est extraite du bloc de la fiche boutique.
#[test]
fn steam_store_icon_is_read_from_the_app_page() {
    let html = r#"<div class="apphub_AppIcon"><img src="https://shared.akamai.steamstatic.com/community_assets/images/apps/620/abc.jpg"></div>"#;
    assert_eq!(
        steam_store_icon_url(html).as_deref(),
        Some("https://shared.akamai.steamstatic.com/community_assets/images/apps/620/abc.jpg")
    );
    assert!(steam_store_icon_url("<html></html>").is_none());
}

/// La clé d'un événement ne dépend pas du nom du jeu, que le daemon peut
/// compléter après coup.
#[test]
fn friend_event_key_ignores_the_game_name() {
    use lapinux_core::social::{SteamFriendEvent, SteamFriendEventKind};

    let mut event = SteamFriendEvent {
        kind: SteamFriendEventKind::GameStarted,
        steam_id: 76561198000000000,
        name: "Alice".into(),
        avatar: String::new(),
        game: String::new(),
        appid: 620,
        at: 1_700_000_000,
    };
    let key = friend_event_key(&event);
    event.game = "Portal 2".into();
    assert_eq!(friend_event_key(&event), key);
}

/// Les corbeilles du panneau s'appuient sur des clés stables, distinctes par
/// type de carte et renouvelées quand Steam publie une nouvelle notification.
#[test]
fn notification_deletion_keys_are_stable_and_distinct() {
    use lapinux_core::social::{SteamNotification, SteamWebNotification};

    let counter = SteamNotification {
        kind: 4,
        count: 2,
        at: 1_700_000_000,
    };
    let counter_key = notification_counter_key(&counter);
    assert_eq!(counter_key, "steam-counter:4:1700000000:2");
    assert_eq!(notification_counter_key(&counter), counter_key);

    let mut web = SteamWebNotification {
        kind: 6,
        title: "Promotion".into(),
        url: "https://store.steampowered.com/".into(),
        at: 1_700_000_100,
        ..Default::default()
    };
    let web_key = notification_web_key(&web);
    assert_eq!(
        web_key,
        "steam-web:6:1700000100:https://store.steampowered.com/"
    );
    // Le libellé peut changer avec la langue : il ne fait pas partie de la
    // clé de suppression.
    web.title = "Sale".into();
    assert_eq!(notification_web_key(&web), web_key);

    let event = SteamFriendEvent::default();
    assert!(friend_notification_key(&event).starts_with("friend:"));
    assert_ne!(friend_notification_key(&event), counter_key);
}

#[test]
fn l_apercu_d_un_message_steam_reste_court_et_lisible() {
    let long = format!(
        "bonjour   [emoticon]steamhappy[/emoticon] {}",
        "a".repeat(100)
    );
    let preview = message_notification_preview(&long);
    assert!(preview.starts_with("bonjour :steamhappy: aaaa"));
    assert_eq!(preview.chars().count(), 78);
    assert!(preview.ends_with('…'));
}

#[test]
fn la_cle_d_un_message_steam_distingue_chaque_envoi() {
    let message = lapinux_core::social::SteamChatMessage {
        from: 222,
        text: "salut".into(),
        at: 1_700_000_000,
        outgoing: false,
        received_live: true,
    };
    assert_eq!(
        steam_message_key(111, &message),
        steam_message_key(111, &message)
    );
    let mut later = message.clone();
    later.at += 1;
    assert_ne!(
        steam_message_key(111, &message),
        steam_message_key(111, &later)
    );
    assert_ne!(
        steam_message_key(111, &message),
        steam_message_key(222, &message)
    );
}

/// L'historique des événements d'amis est relu au démarrage suivant.
#[test]
fn friend_event_history_survives_a_restart() {
    use lapinux_core::social::{SteamFriendEvent, SteamFriendEventKind};

    let root = std::env::temp_dir().join(format!("lapinux-friend-events-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let paths = LapinuxPaths::under(&root);
    let event = SteamFriendEvent {
        kind: SteamFriendEventKind::GameStarted,
        steam_id: 76561198000000000,
        name: "Alice".into(),
        avatar: "https://avatars.steamstatic.com/alice.jpg".into(),
        game: "Portal 2".into(),
        appid: 620,
        at: 1_700_000_000,
    };
    let mut events = VecDeque::new();
    events.push_back(event.clone());
    friend_events_save(&paths, &events);

    let (loaded, seen) = load_friend_event_state(&paths);
    assert_eq!(loaded, events);
    assert!(seen.contains(&friend_event_key(&event)));

    let _ = std::fs::remove_dir_all(&root);
}

/// Chaque case des réglages ne coupe que son propre type d'événement.
#[test]
fn friend_event_preferences_are_independent() {
    use lapinux_core::config::NotificationsConfig;
    use lapinux_core::social::SteamFriendEventKind;

    let mut prefs = NotificationsConfig::default();
    for kind in [
        SteamFriendEventKind::Online,
        SteamFriendEventKind::Offline,
        SteamFriendEventKind::GameStarted,
        SteamFriendEventKind::GameStopped,
    ] {
        assert!(prefs.friend_event_enabled(kind));
    }
    prefs.friend_online = false;
    assert!(!prefs.friend_event_enabled(SteamFriendEventKind::Online));
    assert!(prefs.friend_event_enabled(SteamFriendEventKind::Offline));
    assert!(prefs.friend_event_enabled(SteamFriendEventKind::GameStarted));
    assert!(prefs.friend_event_enabled(SteamFriendEventKind::GameStopped));
    prefs.friend_game_started = false;
    assert!(!prefs.friend_event_enabled(SteamFriendEventKind::GameStarted));
    assert!(prefs.friend_event_enabled(SteamFriendEventKind::GameStopped));

    prefs.enabled = false;
    for kind in [
        SteamFriendEventKind::Online,
        SteamFriendEventKind::Offline,
        SteamFriendEventKind::GameStarted,
        SteamFriendEventKind::GameStopped,
    ] {
        assert!(!prefs.friend_event_enabled(kind));
    }
    prefs.enabled = true;
    assert!(!prefs.friend_event_enabled(SteamFriendEventKind::Online));
    assert!(prefs.friend_event_enabled(SteamFriendEventKind::GameStopped));
}

/// Les catégories de notifications se coupent indépendamment les unes des autres.
#[test]
fn notification_categories_are_independent() {
    use lapinux_core::config::NotificationsConfig;

    let mut prefs = NotificationsConfig::default();
    assert!(prefs.web_notification_enabled(8));
    assert!(prefs.web_notification_enabled(2));
    assert!(prefs.store_notifications_enabled());

    prefs.wishlist_promotions = false;
    assert!(!prefs.web_notification_enabled(8));
    assert!(prefs.web_notification_enabled(2));

    prefs.steam_other = false;
    assert!(!prefs.web_notification_enabled(2));
    assert!(prefs.store_notifications_enabled());

    prefs.store_notifications = false;
    assert!(!prefs.store_notifications_enabled());

    prefs.enabled = false;
    assert!(!prefs.web_notification_enabled(8));
    assert!(!prefs.web_notification_enabled(2));
    assert!(!prefs.store_notifications_enabled());
}

/// La conservation par défaut est d'une semaine et la purge retire les
/// événements d'amis qui la dépassent.
#[test]
fn notification_retention_removes_events_older_than_one_week() {
    use lapinux_core::config::NotificationsConfig;
    use lapinux_core::social::SteamFriendEvent;

    let prefs = NotificationsConfig::default();
    assert_eq!(prefs.retention_days, 7);
    let now = 1_700_000_000;
    let cutoff = notification_cutoff(&prefs, now);
    assert!(notification_is_recent(
        now - 6 * NOTIFICATION_SECONDS_PER_DAY,
        cutoff
    ));
    assert!(!notification_is_recent(
        now - 8 * NOTIFICATION_SECONDS_PER_DAY,
        cutoff
    ));

    let mut old = SteamFriendEvent::default();
    old.at = now - 8 * NOTIFICATION_SECONDS_PER_DAY;
    let mut recent = SteamFriendEvent::default();
    recent.at = now - 6 * NOTIFICATION_SECONDS_PER_DAY;
    let mut history = VecDeque::new();
    history.push_back(old);
    history.push_back(recent);
    assert!(prune_old_friend_events(&mut history, cutoff));
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].at, now - 6 * NOTIFICATION_SECONDS_PER_DAY);
}

#[test]
fn settings_row_controls_stay_inside_the_panel() {
    let ctx = egui::Context::default();
    let mut input = egui::RawInput::default();
    input.screen_rect = Some(egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(760.0, 660.0),
    ));
    let mut rect = None;
    let _ = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            crate::settings_row(ui, "Résolution", "", |ui| {
                rect = Some(
                    ui.add_sized([72.0, 24.0], egui::DragValue::new(&mut 1920_u32))
                        .rect,
                );
            });
        });
    });
    let rect = rect.expect("contrôle dessiné");
    assert!(
        rect.right() >= 744.0,
        "le contrôle doit être collé au bord droit : {rect:?}"
    );
    assert!(
        rect.right() <= 760.0,
        "le contrôle doit rester dans le panneau : {rect:?}"
    );
}

#[test]
fn le_hit_test_egui_local_ignore_les_rectangles_nan() {
    let source = include_str!("../../../vendor/egui-0.29.1/src/hit_test.rs");
    assert!(
        source.contains("w.interact_rect.any_nan()"),
        "le correctif egui #6851 doit rester appliqué"
    );
    assert!(
        source.contains("w.rect.any_nan()")
            && source.contains("let (Some(click_idx), Some(drag_idx))"),
        "le hit-test doit aussi ignorer un rect NaN et ne jamais unwrap"
    );
}

/// Le Puck sans gamepad natif apparaît comme une Steam Controller appairée,
/// avec une clé stable pour garder la sélection quand son nœud evdev change.
#[test]
fn steam_puck_placeholder_is_a_paired_steam_controller() {
    use crate::controllers::{
        is_steam_puck_placeholder, steam_puck_needs_entry, steam_puck_placeholder,
    };

    let device = steam_puck_placeholder();
    assert!(!device.connected);
    assert!(device.gamepad);
    assert!(is_steam_puck_placeholder(&device));
    assert_eq!(device.vendor, 0x28de);
    assert_eq!(device.product, 0x1304);
    assert_eq!(controller_key(&device), "steam-serial:lapinux-steam-puck");
    assert!(steam_puck_needs_entry(true, false, &[]));
    assert!(!steam_puck_needs_entry(false, false, &[]));
    assert!(!steam_puck_needs_entry(true, true, &[]));
}
