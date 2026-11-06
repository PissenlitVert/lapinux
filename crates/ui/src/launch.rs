//! Écran d'attente du lancement : le lapin anime la préparation de la session.
//!
//! La requête au daemon part en tâche de fond, donc l'interface continue de
//! dessiner pendant que Steam prépare Proton et le préfixe. Au bout d'un
//! instant, si le jeu n'est toujours pas là, la fenêtre se couvre : le lapin
//! s'anime au centre, avec le nom du jeu.
//!
//! La planche `assets/bunny-sheet.png` range une animation par ligne, chaque
//! ligne faisant de 2 à 11 images. Toutes sont jouées : une animation tient
//! quelques secondes, puis la suivante prend la main — sans quoi une attente
//! de vingt minutes ne montrerait qu'une boucle.

use super::*;

/// Le dessin fait 32 px de côté : il est agrandi dix fois, avec un filtrage
/// au plus proche voisin pour garder les pixels nets.
const SIDE: f32 = 320.0;
/// Côté d'une image de la planche, en pixels d'image.
///
/// La planche livrée range 11 images par ligne de 32 px : découper en 16 px
/// n'affichait qu'un quart de lapin, une oreille et un bout d'œil.
pub(crate) const CELL: usize = 32;

/// Cases du lapin dans une carte d'attente : la même taille partout, pour que
/// l'attente se ressemble d'un encart à l'autre. Deux cases, parce que les
/// encarts qui attendent n'ont pas tous la même place : un titre pour le
/// résumé, une bande pour les succès. Trois cases débordaient de celui qui en
/// a le moins.
const WAITING_CELLS: f32 = 2.0;
/// Taille du libellé sous le lapin. Un encart qui n'a pas encore sa planche
/// écrit le même texte : la ligne d'attente garde sa taille d'une carte à
/// l'autre, au lieu de changer au moment où le lapin arrive.
pub(crate) const WAITING_LABEL_FONT: f32 = 10.5;
/// Sept images par seconde : le lapin respire au lieu de s'agiter.
const FRAME_SECONDS: f32 = 0.14;
/// Pause de repos entre deux animations : le lapin revient sur sa première
/// image et s'arrête, sans quoi la planche s'enchaîne comme un manège.
pub(crate) const IDLE_SECONDS: f32 = 1.6;
/// En dessous, le lancement est trop rapide pour valoir un écran d'attente.
pub(crate) const DELAY: f32 = 0.6;

/// Une animation de la planche : sa ligne et son nombre d'images.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Animation {
    pub row: usize,
    pub frames: usize,
}

impl Animation {
    /// Durée d'un passage : les images, puis le repos.
    fn cycle(self) -> f32 {
        self.frames.max(1) as f32 * FRAME_SECONDS + IDLE_SECONDS
    }
}

/// Planche du lapin : toutes ses animations, jouées l'une après l'autre.
#[derive(Clone)]
pub(crate) struct Bunny {
    texture: egui::TextureHandle,
    animations: Vec<Animation>,
    /// Taille de la planche, en pixels d'image : les coordonnées de texture
    /// s'y rapportent.
    sheet: [usize; 2],
}

impl Bunny {
    pub fn load(ctx: &egui::Context) -> Option<Self> {
        let image = image::load_from_memory(include_bytes!("../../../assets/bunny-sheet.png"))
            .ok()?
            .to_rgba8();
        let size = [image.width() as usize, image.height() as usize];
        let animations = scan_animations(&image);
        if animations.is_empty() {
            return None;
        }
        let color = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
        // Au plus proche voisin : agrandir un dessin de 16 px en lissant le
        // transformerait en tache floue.
        Some(Self {
            texture: ctx.load_texture("bunny-sheet", color, egui::TextureOptions::NEAREST),
            animations,
            sheet: size,
        })
    }

    /// Nombre d'animations de la planche.
    #[cfg(test)]
    pub fn count(&self) -> usize {
        self.animations.len()
    }

    /// Nombre d'images d'une animation.
    #[cfg(test)]
    pub fn frames_of(&self, index: usize) -> usize {
        self.animations[index].frames
    }

    /// Animation et image affichées après `elapsed` secondes.
    ///
    /// Le temps se découpe en cycles : une animation joue ses images, puis le
    /// lapin se tient immobile sur la première pendant [`IDLE_SECONDS`], puis
    /// l'animation suivante prend la main.
    pub fn scene(&self, elapsed: f32) -> (Animation, usize) {
        let total: f32 = self.animations.iter().map(|animation| animation.cycle()).sum();
        let mut local = elapsed.max(0.0) % total.max(0.001);
        for animation in &self.animations {
            let play = animation.frames.max(1) as f32 * FRAME_SECONDS;
            if local < play {
                let frame = ((local / FRAME_SECONDS) as usize).min(animation.frames - 1);
                return (*animation, frame);
            }
            local -= play;
            if local < IDLE_SECONDS {
                return (*animation, 0);
            }
            local -= IDLE_SECONDS;
        }
        (self.animations[0], 0)
    }

    /// Durée d'un cycle complet de la planche : toutes les animations et leurs
    /// repos.
    #[cfg(test)]
    pub fn cycle_seconds(&self) -> f32 {
        self.animations.iter().map(|animation| animation.cycle()).sum()
    }

    /// Le même lapin à la taille demandée : l'écran de lancement le peint en
    /// grand, les cartes d'attente en petit.
    pub fn draw_sized(
        &self,
        painter: &egui::Painter,
        center: egui::Pos2,
        side: f32,
        elapsed: f32,
    ) {
        self.draw_sized_tinted(painter, center, side, elapsed, egui::Color32::WHITE);
    }

    /// Le lapin teinté : l'écran d'attente s'en sert pour son fondu.
    pub fn draw_sized_tinted(
        &self,
        painter: &egui::Painter,
        center: egui::Pos2,
        side: f32,
        elapsed: f32,
        tint: egui::Color32,
    ) {
        let (animation, frame) = self.scene(elapsed);
        let width = (self.sheet[0] / CELL).max(1) as f32;
        let height = (self.sheet[1] / CELL).max(1) as f32;
        let left = frame as f32 / width;
        let right = (frame + 1) as f32 / width;
        let top = animation.row as f32 / height;
        let bottom = (animation.row + 1) as f32 / height;
        painter.image(
            self.texture.id(),
            egui::Rect::from_center_size(center, egui::vec2(side, side)),
            egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, bottom)),
            tint,
        );
    }
}

/// Découpe la planche en animations : une par ligne, de la première case
/// pleine à la dernière avant un trou. Une ligne vide est ignorée.
fn scan_animations(image: &image::RgbaImage) -> Vec<Animation> {
    let (width, height) = image.dimensions();
    let columns = width as usize / CELL;
    let lines = height as usize / CELL;
    let mut animations = Vec::new();
    for line in 0..lines {
        let mut frames = 0;
        for column in 0..columns {
            if cell_empty(image, column, line) {
                break;
            }
            frames += 1;
        }
        if frames > 0 {
            animations.push(Animation { row: line, frames });
        }
    }
    animations
}

/// Une case de la planche est-elle vide ?
fn cell_empty(image: &image::RgbaImage, column: usize, line: usize) -> bool {
    for y in line * CELL..(line + 1) * CELL {
        for x in column * CELL..(column + 1) * CELL {
            if image.get_pixel(x as u32, y as u32)[3] > 0 {
                return false;
            }
        }
    }
    true
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
    let height = (rect.height() * 0.68).min(rect.width() * 0.72).clamp(180.0, 640.0);
    let width = height * 2.0 / 3.0;
    let capsule = egui::Rect::from_center_size(
        rect.center() - egui::vec2(0.0, rect.height() * 0.045),
        egui::vec2(width, height),
    );
    if let Some(cover) = cover {
        // Fond : la même jaquette agrandie pour couvrir l'écran, très assombrie.
        let scale = (rect.width() / width).max(rect.height() / height);
        let backdrop = egui::Rect::from_center_size(
            rect.center(),
            egui::vec2(width * scale, height * scale),
        );
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
        painter.image(cover.id(), capsule, full, faded(egui::Color32::WHITE, alpha));
    } else {
        // Jeu sans visuel : la tuile garde sa place, avec le titre en grand.
        painter.rect_filled(
            capsule,
            egui::Rounding::same(14.0),
            faded(CARD(), alpha),
        );
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
        let bunny = Bunny::load(&ctx).expect("planche du lapin");
        (ctx, bunny)
    }

    #[test]
    fn the_sheet_holds_many_animations() {
        let (_ctx, bunny) = bunny();
        // La planche du paquet : 34 animations sur 11 colonnes de 32 px.
        assert_eq!(bunny.count(), 34, "{} animations", bunny.count());
        assert_eq!(bunny.frames_of(0), 8, "première animation");
        for index in 0..bunny.count() {
            assert!(
                (2..=11).contains(&bunny.frames_of(index)),
                "animation {index} : {} images",
                bunny.frames_of(index)
            );
        }
    }

    #[test]
    fn une_case_de_la_planche_contient_un_lapin_entier() {
        // Chaque case de 32 px doit porter le dessin sur presque toute sa
        // surface : une case de 16 px ne contiendrait qu'un quart du lapin, et
        // c'est cette découpe qui n'affichait qu'une oreille et un bout d'œil.
        let image = image::load_from_memory(include_bytes!("../../../assets/bunny-sheet.png"))
            .expect("planche")
            .to_rgba8();
        // La signature d'une bonne découpe : aucun dessin ne touche le bord de
        // sa case. Une case trop petite tranche dans le sprite, qui touche
        // alors le bas et la droite.
        let (_ctx, bunny) = bunny();
        let (width, height) = image.dimensions();
        let mut cells = 0;
        for row in 0..height as usize / CELL {
            for column in 0..width as usize / CELL {
                let mut painted = 0;
                for y in 0..CELL {
                    for x in 0..CELL {
                        let px = image.get_pixel(
                            (column * CELL + x) as u32,
                            (row * CELL + y) as u32,
                        );
                        if px[3] == 0 {
                            continue;
                        }
                        painted += 1;
                        assert!(
                            x > 0 && y > 0 && x + 1 < CELL && y + 1 < CELL,
                            "case {column}/{row} : dessin coupé au bord ({x}, {y})"
                        );
                    }
                }
                if painted > 0 {
                    cells += 1;
                }
            }
        }
        // Le compte des cases pleines tombe juste sur la somme des images
        // découpées : la grille et le lecteur voient la même planche.
        let frames: usize = (0..bunny.count()).map(|i| bunny.frames_of(i)).sum();
        assert_eq!(cells, frames, "{cells} cases pleines");
    }

    #[test]
    fn the_bunny_changes_animation_and_loops() {
        let (_ctx, bunny) = bunny();
        let (first, frame) = bunny.scene(0.0);
        assert_eq!(frame, 0);
        // L'image suivante arrive après un battement, pas l'image d'avant.
        let (same, frame) = bunny.scene(FRAME_SECONDS + 0.01);
        assert_eq!(same, first);
        assert_eq!(frame, 1);
        // L'animation joue puis se repose : après le dernier tour, le lapin
        // tient sa première image.
        let play = first.frames as f32 * FRAME_SECONDS;
        let (resting, frame) = bunny.scene(play + IDLE_SECONDS * 0.5);
        assert_eq!(resting, first);
        assert_eq!(frame, 0);
        // Le repos passé, l'animation suivante prend la main.
        let (next, _) = bunny.scene(play + IDLE_SECONDS + 0.01);
        assert_ne!(next, first);
        // La boucle revient au début de la planche.
        let (looped, _) = bunny.scene(bunny.cycle_seconds());
        assert_eq!(looped, first);
    }

    #[test]
    fn le_lapin_r_a_lentit_pas() {
        let (_ctx, bunny) = bunny();
        // Deux images ne passent pas dans la même dixième de seconde : la
        // planche respire, elle ne mitraille pas.
        let (_, frame) = bunny.scene(0.09);
        assert_eq!(frame, 0);
        let (_, frame) = bunny.scene(FRAME_SECONDS);
        assert_eq!(frame, 1);
        // Chaque animation est suivie du même repos.
        let cycle = bunny.cycle_seconds();
        assert!(cycle > 30.0, "cycle de {cycle} s");
    }

    #[test]
    fn les_coordonnees_de_la_case_restent_dans_la_planche() {
        let (_ctx, bunny) = bunny();
        let width = (bunny.sheet[0] / CELL) as f32;
        let height = (bunny.sheet[1] / CELL) as f32;
        // Toute image de toute animation tombe dans une case entière.
        for index in 0..bunny.count() {
            let animation = bunny.animations[index];
            for frame in 0..animation.frames {
                assert!(frame < width as usize, "{index}/{frame}");
                assert!(animation.row < height as usize, "{index}/{frame}");
            }
        }
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
        let mut images = 0usize;
        fn walk(shape: &egui::Shape, texts: &mut Vec<String>, images: &mut usize) {
            match shape {
                egui::Shape::Text(text) => texts.push(text.galley.text().to_string()),
                egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::Managed(0) => {
                    *images += 1;
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, texts, images);
                    }
                }
                _ => {}
            }
        }
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut texts, &mut images);
        }
        assert!(texts.iter().any(|text| text == "Alan Wake"), "{texts:?}");
        assert!(texts.iter().any(|text| text == "Lancement…"), "{texts:?}");
        assert_eq!(images, 1, "une seule image de lapin");
    }
}
