//! Écran d'attente du lancement : le lapin anime la préparation de la session.
//!
//! La requête au daemon part en tâche de fond, donc l'interface continue de
//! dessiner pendant que Steam prépare Proton et le préfixe. Au bout d'un
//! instant, si le jeu n'est toujours pas là, la fenêtre se couvre : le lapin
//! s'anime au centre, avec le nom du jeu.
//!
//! L'indicateur utilise les dessins vectoriels de Lapinux, sans planche externe.

use super::*;

/// Taille de l'indicateur dans l'écran de lancement.
const SIDE: f32 = 320.0;
/// Unité de dimensionnement des indicateurs dans les cartes d'attente.
pub(crate) const CELL: usize = 32;

/// Cases du lapin dans une carte d'attente : la même taille partout, pour que
/// l'attente se ressemble d'un encart à l'autre. Deux cases, parce que les
/// encarts qui attendent n'ont pas tous la même place : un titre pour le
/// résumé, une bande pour les succès. Trois cases débordaient de celui qui en
/// a le moins.
const WAITING_CELLS: f32 = 2.0;
/// Taille du libellé sous l'indicateur dans les cartes d'attente.
pub(crate) const WAITING_LABEL_FONT: f32 = 10.5;
/// En dessous, le lancement est trop rapide pour valoir un écran d'attente.
pub(crate) const DELAY: f32 = 0.6;

/// Indicateur vectoriel stable pendant la vie de l'écran ou de la scène.
#[derive(Clone)]
pub(crate) struct Bunny {
    variant: u8,
}

impl Bunny {
    pub fn load(_ctx: &egui::Context) -> Option<Self> {
        Some(Self {
            variant: crate::section_icons::random_loading_variant(),
        })
    }

    pub fn draw_sized(&self, painter: &egui::Painter, center: egui::Pos2, side: f32, elapsed: f32) {
        self.draw_sized_tinted(painter, center, side, elapsed, egui::Color32::WHITE);
    }

    pub fn draw_sized_tinted(
        &self,
        painter: &egui::Painter,
        center: egui::Pos2,
        side: f32,
        elapsed: f32,
        tint: egui::Color32,
    ) {
        crate::section_icons::paint_loading(painter, center, side, tint, elapsed, self.variant);
    }
}

/// Faut-il déjà couvrir la fenêtre ? Un lancement vif ne fait pas clignoter
/// un écran d'attente pour rien.
pub(crate) fn visible(elapsed: f32) -> bool {
    elapsed >= DELAY
}

/// Grâce après la réponse du daemon avant de couvrir l'écran : un jeu qui
/// affiche sa première image en une fraction de seconde ne doit pas voir un
/// voile apparaître puis disparaître.
pub(crate) const HOLD_GRACE: f32 = 0.3;

/// L'écran doit-il être peint pendant la seconde phase ? Soit il était déjà
/// là pendant la préparation, soit le jeu tarde au-delà de la grâce. Un jeu
/// de boutique répond en quelques millisecondes : sans cette règle, il
/// n'aurait jamais d'écran d'attente, même en chargeant plusieurs secondes.
pub(crate) fn hold_visible(elapsed: f32, since_response: f32) -> bool {
    elapsed >= DELAY || since_response >= HOLD_GRACE
}

/// Écran d'attente choisi dans les réglages généraux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Style {
    /// Jaquette du jeu seule, sans logo qui la traverse.
    Capsule,
    /// Le lapin animé.
    Bunny,
    /// Ne rien couvrir : le jeu prend la fenêtre quand il est prêt.
    Off,
}

impl Style {
    pub fn from_config(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "aucun" | "none" | "off" => Style::Off,
            "lapin" | "bunny" => Style::Bunny,
            _ => Style::Capsule,
        }
    }
}

/// Une couleur rendue transparente : le fondu de l'écran d'attente s'appuie
/// dessus pour chaque trait, texte et image.
fn faded(color: egui::Color32, alpha: f32) -> egui::Color32 {
    let alpha = (color.a() as f32 * alpha.clamp(0.0, 1.0)).round() as u8;
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

/// Écran d'attente « capsule » : la jaquette du jeu au centre et son nom sous
/// l'image, sans logo qui la traverse.
///
/// La jaquette est aussi peinte en fond, assombrie : sans elle, la couverture
/// serait une grande boîte noire autour d'une petite image.
pub(crate) fn paint_capsule(
    painter: &egui::Painter,
    rect: egui::Rect,
    cover: Option<&egui::TextureHandle>,
    name: &str,
    _elapsed: f32,
    alpha: f32,
) {
    let alpha = alpha.clamp(0.0, 1.0);
    let full = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    painter.rect_filled(rect, 0.0, faded(BACKGROUND(), alpha));
    // La capsule garde son format 2:3, quelle que soit la fenêtre.
    let height = (rect.height() * 0.68)
        .min(rect.width() * 0.72)
        .clamp(180.0, 640.0);
    let width = height * 2.0 / 3.0;
    let capsule = egui::Rect::from_center_size(
        rect.center() - egui::vec2(0.0, rect.height() * 0.045),
        egui::vec2(width, height),
    );
    if let Some(cover) = cover {
        // Fond : la même jaquette agrandie pour couvrir l'écran, très assombrie.
        let scale = (rect.width() / width).max(rect.height() / height);
        let backdrop =
            egui::Rect::from_center_size(rect.center(), egui::vec2(width * scale, height * scale));
        painter.image(
            cover.id(),
            backdrop,
            full,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, (alpha * 30.0) as u8),
        );
        // Voile sombre derrière la capsule : le nom reste lisible dessus.
        painter.rect_filled(
            capsule.expand(10.0),
            egui::Rounding::same(16.0),
            faded(egui::Color32::from_black_alpha(170), alpha),
        );
        painter.image(
            cover.id(),
            capsule,
            full,
            faded(egui::Color32::WHITE, alpha),
        );
    } else {
        // Jeu sans visuel : la tuile garde sa place, avec le titre en grand.
        painter.rect_filled(capsule, egui::Rounding::same(14.0), faded(CARD(), alpha));
        painter.text(
            capsule.center(),
            egui::Align2::CENTER_CENTER,
            name.chars().next().unwrap_or('?').to_string(),
            display_font(72.0),
            faded(TEXT_DIM(), alpha * 0.5),
        );
    }
    let text_at = capsule.center_bottom() + egui::vec2(0.0, 24.0);
    painter.text(
        text_at,
        egui::Align2::CENTER_TOP,
        name,
        display_font(21.0),
        faded(TEXT(), alpha),
    );
    painter.text(
        text_at + egui::vec2(0.0, 28.0),
        egui::Align2::CENTER_TOP,
        "Lancement…",
        egui::FontId::proportional(13.0),
        faded(TEXT_DIM(), alpha),
    );
}

/// Lapin d'attente peint dans un rectangle libre : centré, son libellé
/// dessous. Tous les encarts qui lisent Steam s'en servent, pour que
/// l'attente se ressemble d'une carte à l'autre.
pub(crate) fn waiting_in(ui: &egui::Ui, rect: egui::Rect, bunny: &Bunny, label: &str) {
    let elapsed = ui.input(|input| input.time) as f32;
    let cell = CELL as f32;
    // La taille ne dépend que de la largeur : la hauteur libre, elle, change
    // d'un encart à l'autre (titre, bande) et faisait rétrécir le lapin d'une
    // carte à la suivante. Un facteur entier garde les pixels nets.
    let room = (rect.width() - 12.0).max(cell);
    let side = (room / cell).floor().clamp(1.0, WAITING_CELLS) * cell;
    let center = rect.center() - egui::vec2(0.0, 8.0);
    bunny.draw_sized(ui.painter(), center, side, elapsed);
    ui.painter().text(
        center + egui::vec2(0.0, side * 0.5 + 2.0),
        egui::Align2::CENTER_TOP,
        label,
        egui::FontId::proportional(WAITING_LABEL_FONT),
        TEXT_DIM(),
    );
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(100));
}

/// Voile plein cadre : le lapin au centre, le nom du jeu et l'attente.
pub(crate) fn paint(
    painter: &egui::Painter,
    rect: egui::Rect,
    bunny: &Bunny,
    name: &str,
    elapsed: f32,
    alpha: f32,
) {
    let alpha = alpha.clamp(0.0, 1.0);
    painter.rect_filled(rect, 0.0, faded(BACKGROUND(), alpha));
    let center = rect.center() + egui::vec2(0.0, -40.0);
    bunny.draw_sized_tinted(
        painter,
        center,
        SIDE,
        elapsed,
        faded(egui::Color32::WHITE, alpha),
    );
    painter.text(
        center + egui::vec2(0.0, 190.0),
        egui::Align2::CENTER_CENTER,
        name,
        display_font(21.0),
        faded(TEXT(), alpha),
    );
    painter.text(
        center + egui::vec2(0.0, 218.0),
        egui::Align2::CENTER_CENTER,
        "Lancement…",
        egui::FontId::proportional(13.0),
        faded(TEXT_DIM(), alpha),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bunny() -> (egui::Context, Bunny) {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let bunny = Bunny::load(&ctx).expect("indicateur de chargement");
        (ctx, bunny)
    }

    #[test]
    fn saved_bunny_style_names_remain_compatible() {
        assert_eq!(Style::from_config("lapin"), Style::Bunny);
        assert_eq!(Style::from_config("bunny"), Style::Bunny);
    }

    #[test]
    fn a_quick_launch_does_not_cover_the_window() {
        assert!(!visible(0.0));
        assert!(!visible(DELAY - 0.01));
        assert!(visible(DELAY));
    }

    #[test]
    fn a_fast_daemon_answer_still_gets_a_waiting_screen() {
        // Réponse en 30 ms : l'écran ne doit pas apparaître tout de suite,
        // mais il doit exister si le jeu n'a toujours rien affiché.
        assert!(!hold_visible(0.03, 0.0));
        assert!(!hold_visible(0.03, HOLD_GRACE - 0.01));
        assert!(hold_visible(0.03, HOLD_GRACE));
        // Une préparation lente avait déjà couvert la fenêtre : l'écran
        // reste peint pendant la seconde phase.
        assert!(hold_visible(DELAY, 0.0));
    }

    #[test]
    fn the_waiting_screen_paints_the_bunny_and_the_name() {
        let (ctx, bunny) = bunny();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0));
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    egui::Id::new("launch-test"),
                ));
                paint(&painter, screen, &bunny, "Alan Wake", 1.2, 1.0);
            },
        );
        let mut texts = Vec::new();
        let mut paths = 0usize;
        fn walk(shape: &egui::Shape, texts: &mut Vec<String>, paths: &mut usize) {
            match shape {
                egui::Shape::Text(text) => texts.push(text.galley.text().to_string()),
                egui::Shape::Path(_) | egui::Shape::Circle(_) => {
                    *paths += 1;
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, texts, paths);
                    }
                }
                _ => {}
            }
        }
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut texts, &mut paths);
        }
        assert!(texts.iter().any(|text| text == "Alan Wake"), "{texts:?}");
        assert!(texts.iter().any(|text| text == "Lancement…"), "{texts:?}");
        assert!(paths > 0, "un indicateur vectoriel doit être dessiné");
    }
}
