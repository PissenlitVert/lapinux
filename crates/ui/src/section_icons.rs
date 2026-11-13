use super::{egui, Page, SettingsTab};

/// Taille commune des pictogrammes de la barre classique et du rail Réglages.
pub(super) const INTERFACE_ICON_SIZE: f32 = 19.0;

// Dessins vectoriels sur une grille de 24 unités, partagée par tous les thèmes.
struct Icon<'a> {
    painter: &'a egui::Painter,
    center: egui::Pos2,
    scale: f32,
    rotation: f32,
    stroke: egui::Stroke,
    amount: f32,
}

impl<'a> Icon<'a> {
    fn new(
        painter: &'a egui::Painter,
        center: egui::Pos2,
        size: f32,
        color: egui::Color32,
        progress: f32,
    ) -> Self {
        let scale = size / 24.0;
        Self {
            painter,
            center,
            scale,
            rotation: 0.0,
            stroke: egui::Stroke::new(1.6 * scale, color),
            amount: (progress.clamp(0.0, 1.0) * std::f32::consts::PI).sin(),
        }
    }
    fn p(&self, x: f32, y: f32) -> egui::Pos2 {
        let (sin, cos) = self.rotation.sin_cos();
        let (x, y) = (x - 12.0, y - 12.0);
        self.center + egui::vec2(x * cos - y * sin, x * sin + y * cos) * self.scale
    }
    fn moved(&self, x: f32, y: f32) -> Self {
        Self {
            painter: self.painter,
            center: self.center + egui::vec2(x, y) * self.scale,
            scale: self.scale,
            rotation: self.rotation,
            stroke: self.stroke,
            amount: self.amount,
        }
    }
    fn path(&self, points: &[(f32, f32)]) {
        self.painter.add(egui::Shape::line(
            points.iter().map(|&(x, y)| self.p(x, y)).collect(),
            self.stroke,
        ));
    }
    fn rect(&self, x: f32, y: f32, w: f32, h: f32, radius: f32) {
        self.painter.rect_stroke(
            egui::Rect::from_min_size(self.p(x, y), egui::vec2(w, h) * self.scale),
            egui::Rounding::same(radius * self.scale),
            self.stroke,
        );
    }
    fn circle(&self, x: f32, y: f32, r: f32) {
        self.painter
            .circle_stroke(self.p(x, y), r * self.scale, self.stroke);
    }
    fn dot(&self, x: f32, y: f32, r: f32) {
        self.painter
            .circle_filled(self.p(x, y), r * self.scale, self.stroke.color);
    }
    fn curve(&self, points: [(f32, f32); 4]) {
        let points = (0..=16)
            .map(|i| {
                let t = i as f32 / 16.0;
                let u = 1.0 - t;
                self.p(
                    u * u * u * points[0].0
                        + 3.0 * u * u * t * points[1].0
                        + 3.0 * u * t * t * points[2].0
                        + t * t * t * points[3].0,
                    u * u * u * points[0].1
                        + 3.0 * u * u * t * points[1].1
                        + 3.0 * u * t * t * points[2].1
                        + t * t * t * points[3].1,
                )
            })
            .collect();
        self.painter.add(egui::Shape::line(points, self.stroke));
    }
    fn home(&self) {
        let house = self.moved(0.0, -2.0 * self.amount);
        house.path(&[(3.5, 11.0), (12.0, 4.0), (20.5, 11.0)]);
        house.path(&[(6.0, 10.0), (6.0, 20.0), (18.0, 20.0), (18.0, 10.0)]);
        house.path(&[(10.0, 20.0), (10.0, 14.0), (14.0, 14.0), (14.0, 20.0)]);
    }
    fn library(&self, settings: bool) {
        let first = self.moved(0.0, if settings { -2.0 * self.amount } else { 0.0 });
        first.rect(3.0, 5.5, 4.0, 13.0, 0.0);
        first.path(&[(3.0, 15.5), (7.0, 15.5)]);
        let book = self.moved(0.0, if settings { 0.0 } else { -3.0 * self.amount });
        book.rect(9.0, 5.5, 4.0, 13.0, 0.0);
        book.path(&[(9.0, 15.5), (13.0, 15.5)]);
        let lean = if settings { 1.8 * self.amount } else { 0.0 };
        self.path(&[
            (15.0 + lean, 6.0),
            (18.5 + lean, 5.0),
            (21.5, 17.5),
            (18.0, 18.5),
            (15.0 + lean, 6.0),
        ]);
    }
    fn collections(&self) {
        self.rect(5.0, 6.0, 14.0, 14.0, 2.0);
        self.path(&[(8.0, 3.0), (19.0, 3.0), (21.0, 5.0), (21.0, 16.0)]);
        let a = self.amount * 0.42;
        let (sn, cs) = a.sin_cos();
        let points = [
            (12.0, 9.5),
            (13.2, 11.8),
            (15.8, 12.2),
            (13.9, 14.0),
            (14.4, 16.6),
            (12.0, 15.3),
            (9.6, 16.6),
            (10.1, 14.0),
            (8.2, 12.2),
            (10.8, 11.8),
            (12.0, 9.5),
        ];
        let v: Vec<_> = points
            .iter()
            .map(|&(x, y)| {
                let x = x - 12.0;
                let y = y - 13.0;
                (12.0 + x * cs - y * sn, 13.0 + x * sn + y * cs)
            })
            .collect();
        self.path(&v);
    }
    fn news(&self) {
        self.rect(5.0, 4.0, 14.0, 16.0, 0.0);
        self.path(&[(3.0, 7.0), (3.0, 20.0), (17.0, 20.0)]);
        for (index, (y, end)) in [(8.0, 16.0), (11.0, 16.0), (14.0, 13.0), (17.0, 15.0)]
            .into_iter()
            .enumerate()
        {
            // Les titres se déploient en cascade, comme un fil qui se charge.
            let length = (end - 8.0) * (1.0 - 0.38 * self.amount * (index as f32 + 1.0) / 4.0);
            self.path(&[(8.0, y), (8.0 + length, y)]);
        }
    }
    fn downloads(&self) {
        let arrow = self.moved(0.0, 2.0 * self.amount);
        arrow.path(&[(12.0, 3.0), (12.0, 14.0)]);
        arrow.path(&[(8.0, 10.0), (12.0, 14.0), (16.0, 10.0)]);
        self.path(&[
            (4.0, 15.0),
            (4.0, 18.0),
            (5.0, 20.0),
            (19.0, 20.0),
            (20.0, 18.0),
            (20.0, 15.0),
        ]);
    }
    fn controllers(&self) {
        // La manette pivote comme lors d'une impulsion, puis revient au repos.
        let mut controller = self.moved(0.0, -0.5 * self.amount);
        controller.rotation = -0.14 * self.amount;
        controller.controller_body();
        if self.amount > 0.001 {
            // Deux petits traits évoquent la vibration sans encombrer les boutons.
            let marks = Icon::new(
                self.painter,
                self.center,
                self.scale * 24.0,
                self.stroke.color.gamma_multiply(self.amount),
                0.0,
            );
            marks.path(&[(0.5, 9.5), (0.5, 12.5)]);
            marks.path(&[(23.5, 9.5), (23.5, 12.5)]);
        }
    }
    fn controller_body(&self) {
        self.curve([(7.5, 6.0), (5.5, 6.0), (4.4, 7.6), (3.7, 10.4)]);
        self.path(&[(3.7, 10.4), (2.7, 14.8)]);
        self.curve([(2.7, 14.8), (1.9, 18.1), (4.4, 20.0), (6.2, 17.8)]);
        self.path(&[(6.2, 17.8), (8.0, 15.4), (16.0, 15.4), (17.8, 17.8)]);
        self.curve([(17.8, 17.8), (19.6, 20.0), (22.1, 18.1), (21.3, 14.8)]);
        self.curve([(21.3, 14.8), (20.5, 11.0), (20.4, 6.0), (16.5, 6.0)]);
        self.path(&[(16.5, 6.0), (7.5, 6.0)]);
        self.path(&[(6.5, 10.4), (10.5, 10.4)]);
        self.path(&[(8.5, 8.4), (8.5, 12.4)]);
        self.dot(15.5, 9.8, 0.85);
        self.dot(18.0, 12.3, 0.85);
    }
    fn profile(&self) {
        self.moved(0.0, -1.5 * self.amount).circle(12.0, 8.0, 3.5);
        self.path(&[(5.0, 20.0), (5.0, 18.5)]);
        self.curve([(5.0, 18.5), (5.0, 11.2), (19.0, 11.2), (19.0, 18.5)]);
        self.path(&[(19.0, 18.5), (19.0, 20.0), (5.0, 20.0)]);
    }
    fn stores(&self) {
        self.path(&[(4.0, 10.0), (4.0, 19.0), (20.0, 19.0), (20.0, 10.0)]);
        self.path(&[
            (3.0, 6.0),
            (4.0, 3.0),
            (20.0, 3.0),
            (21.0, 6.0),
            (21.0, 8.0),
        ]);
        self.curve([(21.0, 8.0), (21.0, 10.3), (18.8, 10.7), (17.4, 9.7)]);
        self.curve([(17.4, 9.7), (16.4, 10.7), (15.0, 10.7), (14.0, 9.7)]);
        self.curve([(14.0, 9.7), (13.0, 10.7), (11.6, 10.7), (10.6, 9.7)]);
        self.curve([(10.6, 9.7), (9.6, 10.7), (8.2, 10.7), (7.2, 9.7)]);
        self.curve([(7.2, 9.7), (5.8, 10.7), (3.0, 10.3), (3.0, 8.0)]);
        self.path(&[(3.0, 8.0), (3.0, 6.0)]);
        // Le cadre reste en place ; le battant pivote vers son bord gauche.
        self.path(&[(7.0, 19.0), (7.0, 14.0), (11.0, 14.0), (11.0, 19.0)]);
        if self.amount > 0.001 {
            let edge = 11.0 - 2.5 * self.amount;
            self.path(&[(7.0, 14.0), (edge, 14.0 + 0.8 * self.amount), (edge, 19.0)]);
        }
    }
    fn captures(&self, settings: bool) {
        self.rect(3.0, 4.0, 18.0, 16.0, 2.0);
        self.circle(
            8.0,
            9.0,
            1.5 + if settings { 0.0 } else { 1.3 * self.amount },
        );
        let lift = if settings { 1.8 * self.amount } else { 0.0 };
        self.path(&[
            (4.0, 17.0),
            (9.0, 12.0 - lift),
            (12.5, 15.5),
            (15.0, 13.0 - lift),
            (20.0, 18.0),
        ]);
    }
    fn general(&self) {
        self.rect(3.0, 4.0, 18.0, 16.0, 2.0);
        self.path(&[(3.0, 8.0), (21.0, 8.0)]);
        self.path(&[(7.0, 12.0), (11.0, 12.0)]);
        self.path(&[(7.0, 16.0), (14.0, 16.0)]);
        self.moved(0.0, 2.0 * self.amount).dot(16.5, 12.0, 0.7);
        self.dot(16.5, 16.0, 0.7);
    }
    /// Puce graphique : boîtier, ventilateur, ailettes et pattes. Distinct
    /// de la palette d’Apparence, qui partageait le même dessin.
    fn graphics(&self) {
        let lift = self.moved(0.0, -1.2 * self.amount);
        lift.rect(4.0, 7.0, 16.0, 10.5, 2.0);
        lift.circle(12.0, 12.2, 2.4);
        for (x, y) in [(7.0, 10.0), (17.0, 10.0), (7.0, 14.4), (17.0, 14.4)] {
            lift.path(&[(x - 0.7, y), (x + 0.7, y)]);
        }
        for x in [8.5, 12.0, 15.5] {
            lift.path(&[(x, 7.0), (x, 4.2)]);
            lift.path(&[(x, 17.5), (x, 20.3)]);
        }
    }
    fn appearance(&self) {
        // La palette reste reconnaissable pendant l'apparition du pinceau.
        let palette = Icon::new(
            self.painter,
            self.center,
            self.scale * 24.0,
            self.stroke.color.gamma_multiply(1.0 - 0.45 * self.amount),
            0.0,
        );
        palette.appearance_palette();
        if self.amount > 0.001 {
            let brush = Icon::new(
                self.painter,
                self.center,
                self.scale * 24.0,
                self.stroke.color.gamma_multiply(self.amount),
                0.0,
            );
            // Manche et poils dessinés en diagonale, dans le cadre de l'icône.
            brush.path(&[
                (12.0, 14.5),
                (18.5, 7.5),
                (20.0, 9.0),
                (13.5, 16.0),
                (12.0, 14.5),
            ]);
            brush.curve([(12.0, 14.5), (10.5, 14.7), (10.0, 16.8), (9.0, 17.5)]);
            brush.curve([(9.0, 17.5), (11.1, 17.6), (13.2, 17.6), (13.5, 16.0)]);
        }
    }
    fn appearance_palette(&self) {
        self.curve([(12.0, 3.0), (7.0, 3.0), (3.0, 7.0), (3.0, 12.0)]);
        self.curve([(3.0, 12.0), (3.0, 17.0), (7.0, 21.0), (12.0, 21.0)]);
        self.path(&[(12.0, 21.0), (13.0, 21.0)]);
        self.curve([(13.0, 21.0), (15.0, 21.0), (15.7, 19.2), (14.6, 17.8)]);
        self.curve([(14.6, 17.8), (13.5, 16.3), (14.2, 15.0), (16.0, 15.0)]);
        self.path(&[(16.0, 15.0), (17.5, 15.0)]);
        self.curve([(17.5, 15.0), (20.0, 15.0), (21.0, 13.5), (21.0, 11.5)]);
        self.curve([(21.0, 11.5), (21.0, 6.8), (17.0, 3.0), (12.0, 3.0)]);
        for (x, y) in [(7.5, 11.0), (10.0, 7.5), (14.0, 7.5), (17.0, 10.0)] {
            self.dot(x, y, 0.85);
        }
    }
    fn accounts(&self) {
        self.circle(9.0, 8.0, 3.0);
        self.path(&[(3.5, 19.0), (3.5, 18.0)]);
        self.curve([(3.5, 18.0), (3.5, 12.0), (14.5, 12.0), (14.5, 18.0)]);
        self.path(&[(14.5, 18.0), (14.5, 19.0), (3.5, 19.0)]);
        let other = self.moved(1.6 * self.amount, 0.0);
        other.curve([(16.0, 5.5), (19.8, 6.0), (19.8, 10.8), (16.0, 11.3)]);
        other.curve([(16.5, 14.0), (19.0, 14.3), (20.5, 15.7), (20.5, 18.0)]);
        other.path(&[(20.5, 18.0), (20.5, 19.0)]);
    }
    fn storage(&self) {
        self.rect(3.0, 5.0, 18.0, 14.0, 2.0);
        self.path(&[(3.0, 15.0), (21.0, 15.0)]);
        self.dot(7.0, 17.0, 0.7);
        self.dot(10.0, 17.0, 0.7 + self.amount);
    }
    fn saves(&self) {
        self.path(&[
            (4.0, 3.0),
            (17.0, 3.0),
            (20.0, 6.0),
            (20.0, 20.0),
            (4.0, 20.0),
            (4.0, 3.0),
        ]);
        self.path(&[
            (7.0, 3.0),
            (7.0, 9.0),
            (16.0 - 3.0 * self.amount, 9.0),
            (16.0 - 3.0 * self.amount, 3.0),
        ]);
        self.path(&[(7.0, 20.0), (7.0, 13.0), (17.0, 13.0), (17.0, 20.0)]);
        self.path(&[(13.0, 5.0), (13.0, 7.0)]);
    }
    fn protons(&self) {
        self.circle(12.0, 12.0, 1.5);
        for base in [
            0.0_f32,
            std::f32::consts::PI / 3.0,
            2.0 * std::f32::consts::PI / 3.0,
        ] {
            let a = base + 0.65 * self.amount;
            let points: Vec<_> = (0..=48)
                .map(|i| {
                    let t = i as f32 * std::f32::consts::TAU / 48.0;
                    let x = 9.0 * t.cos();
                    let y = 4.0 * t.sin();
                    (
                        12.0 + x * a.cos() - y * a.sin(),
                        12.0 + x * a.sin() + y * a.cos(),
                    )
                })
                .collect();
            self.path(&points);
        }
    }
    fn system(&self) {
        self.rect(6.0, 6.0, 12.0, 12.0, 2.0);
        let d = 0.7 * self.amount;
        self.rect(9.0 - d, 9.0 - d, 6.0 + 2.0 * d, 6.0 + 2.0 * d, 0.5);
        for p in [9.0, 15.0] {
            self.path(&[(p, 3.0), (p, 6.0)]);
            self.path(&[(p, 18.0), (p, 21.0)]);
            self.path(&[(3.0, p), (6.0, p)]);
            self.path(&[(18.0, p), (21.0, p)]);
        }
    }
    fn doctor(&self) {
        self.rect(3.0, 4.0, 18.0, 16.0, 2.0);
        let d = self.amount;
        self.path(&[
            (5.0, 12.0),
            (8.0, 12.0),
            (9.5, 9.0 - d),
            (12.5, 15.0 + d),
            (14.5, 11.0 - d * 0.3),
            (15.5, 12.0),
            (19.0, 12.0),
        ]);
    }
    fn logs(&self) {
        self.path(&[
            (5.0, 3.0),
            (16.0, 3.0),
            (19.0, 6.0),
            (19.0, 21.0),
            (5.0, 21.0),
            (5.0, 3.0),
        ]);
        self.path(&[(15.0, 3.0), (15.0, 7.0), (19.0, 7.0)]);
        for (index, (y, end)) in [(10.0, 16.0), (13.0, 16.0), (16.0, 14.0)]
            .into_iter()
            .enumerate()
        {
            let shift = self.amount * if index == 1 { 1.8 } else { -1.2 };
            self.path(&[(8.0 + shift, y), (end + shift, y)]);
        }
    }
    fn application(&self) {
        let bend = 1.0 * self.amount;
        self.curve([
            (8.0, 9.44),
            (6.0 - bend, 7.0),
            (5.5 - bend, 3.0),
            (7.0 - bend, 3.0),
        ]);
        self.curve([
            (7.0 - bend, 3.0),
            (8.6 - bend, 3.0),
            (10.2, 6.2),
            (10.5, 8.78),
        ]);
        self.curve([
            (16.0, 9.42),
            (18.0 + bend, 7.0),
            (18.5 + bend, 3.0),
            (17.0 + bend, 3.0),
        ]);
        self.curve([
            (17.0 + bend, 3.0),
            (15.4 + bend, 3.0),
            (13.8, 6.2),
            (13.5, 8.79),
        ]);
        self.curve([(7.0, 10.0), (5.3, 11.1), (4.5, 12.8), (4.5, 14.5)]);
        self.curve([(4.5, 14.5), (4.5, 18.3), (7.8, 21.0), (12.0, 21.0)]);
        self.curve([(12.0, 21.0), (16.2, 21.0), (19.5, 18.3), (19.5, 14.5)]);
        self.curve([(19.5, 14.5), (19.5, 10.5), (16.0, 8.7), (12.0, 8.7)]);
        self.curve([(12.0, 8.7), (10.0, 8.7), (8.3, 9.1), (7.0, 10.0)]);
        self.dot(9.0, 14.0, 0.8);
        self.dot(15.0, 14.0, 0.8);
        self.path(&[(11.0, 17.0), (13.0, 17.0)]);
    }
}

pub(super) fn page_animated(
    painter: &egui::Painter,
    page: Page,
    center: egui::Pos2,
    size: f32,
    ink: egui::Color32,
    progress: f32,
) {
    let i = Icon::new(painter, center, size, ink, progress);
    match page {
        Page::Home => i.home(),
        Page::Library => i.library(false),
        Page::Collections => i.collections(),
        Page::News => i.news(),
        Page::Downloads => i.downloads(),
        Page::Controllers => i.controllers(),
        Page::Profile => i.profile(),
        Page::Boutiques => i.stores(),
        Page::Captures => i.captures(false),
    }
}
pub(super) fn page(
    painter: &egui::Painter,
    page: Page,
    center: egui::Pos2,
    size: f32,
    ink: egui::Color32,
) {
    page_animated(painter, page, center, size, ink, 0.0);
}
pub(super) fn settings_tab_animated(
    painter: &egui::Painter,
    tab: SettingsTab,
    center: egui::Pos2,
    size: f32,
    ink: egui::Color32,
    progress: f32,
) {
    let i = Icon::new(painter, center, size, ink, progress);
    match tab {
        SettingsTab::General => i.general(),
        SettingsTab::Notifications => notifications(painter, center, size, ink, progress),
        SettingsTab::InGame => i.controllers(),
        SettingsTab::Appearance => i.appearance(),
        SettingsTab::Library => i.library(true),
        SettingsTab::Connection => i.accounts(),
        SettingsTab::Storage => i.storage(),
        SettingsTab::Saves => i.saves(),
        SettingsTab::Protons => i.protons(),
        SettingsTab::Graphics => i.graphics(),
        SettingsTab::Captures => i.captures(true),
        SettingsTab::System => i.system(),
        SettingsTab::Doctor => i.doctor(),
        SettingsTab::Logs => i.logs(),
        SettingsTab::Updates => i.downloads(),
        SettingsTab::Application => i.application(),
    }
}
pub(super) fn gear(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    ink: egui::Color32,
    progress: f32,
) {
    let i = Icon::new(painter, center, size, ink, progress);
    let mut points = Vec::with_capacity(33);
    for tooth in 0..8 {
        let base =
            tooth as f32 * std::f32::consts::TAU / 8.0 + i.amount * std::f32::consts::FRAC_PI_4;
        for (offset, radius) in [(-0.5_f32, 7.2_f32), (-0.34, 9.2), (0.34, 9.2), (0.5, 7.2)] {
            let a = base + offset * std::f32::consts::TAU / 8.0;
            points.push((12.0 + radius * a.cos(), 12.0 + radius * a.sin()));
        }
    }
    points.push(points[0]);
    i.path(&points);
    i.circle(12.0, 12.0, 3.1);
}

pub(super) fn friends(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    ink: egui::Color32,
    progress: f32,
) {
    let i = Icon::new(painter, center, size, ink, progress);
    // Deux avatars séparés et leurs épaules : aucune silhouette ne masque l'autre.
    let left = i.moved(-0.7 * i.amount, 0.0);
    let right = i.moved(0.7 * i.amount, 0.0);
    left.circle(7.5, 8.0, 2.8);
    left.curve([(3.0, 18.5), (3.0, 12.5), (12.0, 12.5), (12.0, 18.5)]);
    right.circle(16.5, 8.0, 2.8);
    right.curve([(12.0, 18.5), (12.0, 12.5), (21.0, 12.5), (21.0, 18.5)]);
    i.path(&[(3.0, 19.0), (21.0, 19.0)]);
}
/// Mégaphone avec ondes courtes, animé au survol et au focus.
pub(super) fn notifications(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    ink: egui::Color32,
    progress: f32,
) {
    let i = Icon::new(painter, center, size, ink, progress);
    i.path(&[
        (3.0, 10.0),
        (7.0, 10.0),
        (16.0, 6.0),
        (16.0, 18.0),
        (7.0, 14.0),
        (3.0, 14.0),
        (3.0, 10.0),
    ]);
    i.path(&[(7.0, 14.0), (8.4, 19.0), (11.0, 19.0), (9.8, 15.3)]);
    // Une première onde reste visible ; la seconde apparaît puis s'efface.
    i.curve([(18.0, 9.0), (20.5, 10.5), (20.5, 13.5), (18.0, 15.0)]);
    if i.amount > 0.001 {
        let wave = Icon::new(
            painter,
            center,
            size,
            super::with_alpha(ink, i.amount),
            progress,
        );
        wave.curve([(20.5, 7.0), (24.0, 9.5), (24.0, 14.5), (20.5, 17.0)]);
        // Les extrémités arrondies rendent l'onde lisible même à petite taille.
        wave.dot(20.5, 7.0, 0.8);
        wave.dot(20.5, 17.0, 0.8);
    }
}

pub(super) fn fullscreen(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    ink: egui::Color32,
    progress: f32,
    active: bool,
) {
    let i = Icon::new(painter, center, size, ink, progress);
    let d = i.amount;
    if active {
        for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            i.path(&[
                (12.0 + sx * (5.0 - d), 12.0 + sy * (2.0 - d)),
                (12.0 + sx * (5.0 - d), 12.0 + sy * (5.0 - d)),
                (12.0 + sx * (2.0 - d), 12.0 + sy * (5.0 - d)),
            ]);
            i.path(&[
                (12.0 + sx * 8.0, 12.0 + sy * 8.0),
                (12.0 + sx * (5.0 - d), 12.0 + sy * (5.0 - d)),
            ]);
        }
    } else {
        for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            i.path(&[
                (12.0 + sx * (5.0 + d), 12.0 + sy * (9.0 + d)),
                (12.0 + sx * (9.0 + d), 12.0 + sy * (9.0 + d)),
                (12.0 + sx * (9.0 + d), 12.0 + sy * (5.0 + d)),
            ]);
            i.path(&[
                (12.0 + sx * 3.5, 12.0 + sy * 3.5),
                (12.0 + sx * (7.0 + d), 12.0 + sy * (7.0 + d)),
            ]);
        }
    }
}

// Une variante est tirée à l'apparition et conservée tant que ce widget est présent.
const LOADING_BUNNY_VARIANTS: u8 = 5;

#[derive(Clone, Copy, Debug)]
struct LoadingBunny {
    last_frame: u64,
    variant: u8,
}

fn loading_choice(
    previous: Option<LoadingBunny>,
    frame: u64,
    draw: impl FnOnce() -> u8,
) -> LoadingBunny {
    let variant = match previous {
        Some(state) if frame.saturating_sub(state.last_frame) <= 1 => state.variant,
        _ => draw() % LOADING_BUNNY_VARIANTS,
    };
    LoadingBunny {
        last_frame: frame,
        variant,
    }
}

fn random_bunny() -> u8 {
    use std::hash::{BuildHasher, Hasher};
    // RandomState se fournit auprès du système, sans dépendance supplémentaire.
    (std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
        % u64::from(LOADING_BUNNY_VARIANTS)) as u8
}

fn loading_star(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    ink: egui::Color32,
    x: f32,
    y: f32,
    radius: f32,
    opacity: f32,
) {
    let color = egui::Color32::from_rgba_unmultiplied(
        ink.r(),
        ink.g(),
        ink.b(),
        (ink.a() as f32 * opacity.clamp(0.0, 1.0)) as u8,
    );
    let i = Icon::new(painter, center, size, color, 0.0);
    i.path(&[(x - radius, y), (x + radius, y)]);
    i.path(&[(x, y - radius), (x, y + radius)]);
}

/// Dessine les cinq indicateurs choisis, également utilisés pour les grands écrans.
pub(crate) fn paint_loading(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    ink: egui::Color32,
    time: f32,
    variant: u8,
) {
    let phase = (time / 1.4).rem_euclid(1.0);
    let twinkle = (1.0 - (phase * std::f32::consts::TAU).cos()) * 0.5;
    let bunny = Icon::new(painter, center, size, ink, 0.0);
    // Les extrémités des oreilles rejoignent exactement le contour du visage.
    // AE : les oreilles se balancent sans détacher leur base du visage.
    let sway = if variant == 4 {
        (phase * std::f32::consts::TAU).sin() * 1.0
    } else {
        0.0
    };
    bunny.curve([
        (8.1, 10.1),
        (6.0 + sway, 6.1),
        (6.5 + sway, 2.0),
        (8.3 + sway, 2.0),
    ]);
    bunny.curve([
        (8.3 + sway, 2.0),
        (10.0 + sway, 2.0),
        (10.8 + sway, 6.3),
        (10.9, 9.0),
    ]);
    bunny.curve([
        (15.9, 10.1),
        (18.0 + sway, 6.1),
        (17.5 + sway, 2.0),
        (15.7 + sway, 2.0),
    ]);
    bunny.curve([
        (15.7 + sway, 2.0),
        (14.0 + sway, 2.0),
        (13.2 + sway, 6.3),
        (13.1, 9.0),
    ]);
    bunny.curve([(8.1, 10.1), (3.4, 12.4), (5.0, 21.0), (12.0, 21.0)]);
    bunny.curve([(12.0, 21.0), (19.0, 21.0), (20.6, 12.4), (15.9, 10.1)]);
    bunny.curve([(10.9, 9.0), (11.6, 8.6), (12.4, 8.6), (13.1, 9.0)]);
    if variant == 2 {
        bunny.path(&[(8.7, 15.0), (9.5, 15.4), (10.3, 15.0)]);
        bunny.path(&[(13.7, 15.0), (14.5, 15.4), (15.3, 15.0)]);
    } else {
        bunny.dot(9.4, 15.0, 0.65);
        let blink = variant == 3 && phase > 0.42 && phase < 0.54;
        if blink {
            bunny.path(&[(13.8, 15.0), (15.2, 15.0)]);
        } else {
            bunny.dot(14.6, 15.0, 0.65);
        }
    }
    bunny.path(&[(11.2, 17.2), (12.0, 17.8), (12.8, 17.2)]);
    match variant {
        // M : étoiles jumelles en alternance.
        0 => {
            loading_star(
                painter,
                center,
                size,
                ink,
                2.5,
                7.0,
                1.4,
                0.35 + 0.65 * twinkle,
            );
            loading_star(
                painter,
                center,
                size,
                ink,
                21.5,
                15.0,
                1.4,
                1.0 - 0.65 * twinkle,
            );
        }
        // Q : couronne de trois étoiles.
        1 => {
            for (x, y, r) in [(12.0, 2.0, 1.3), (2.8, 7.0, 1.0), (21.2, 7.0, 1.0)] {
                loading_star(painter, center, size, ink, x, y, r, 0.35 + 0.65 * twinkle);
            }
        }
        // R : lune et étoile flottantes.
        2 => {
            let moon = bunny.moved(0.0, -0.5 * twinkle);
            moon.curve([(19.7, 4.0), (18.7, 6.3), (20.3, 8.3), (22.0, 8.4)]);
            moon.curve([(22.0, 8.4), (20.1, 8.8), (18.4, 7.0), (19.7, 4.0)]);
            loading_star(
                painter,
                center,
                size,
                ink,
                2.8,
                8.5,
                1.3,
                0.45 + 0.55 * twinkle,
            );
        }
        // S : clin d'œil et étoile complice.
        3 => loading_star(
            painter,
            center,
            size,
            ink,
            21.0,
            8.0,
            1.6,
            0.35 + 0.65 * twinkle,
        ),
        // AE : deux étoiles scintillent au rythme des oreilles.
        4 => {
            for x in [2.8, 21.2] {
                loading_star(
                    painter,
                    center,
                    size,
                    ink,
                    x,
                    6.5,
                    1.3,
                    0.35 + 0.65 * twinkle,
                );
            }
        }
        _ => unreachable!("unknown loading variant"),
    }
}

pub(crate) fn random_loading_variant() -> u8 {
    random_bunny()
}

pub(crate) fn loading(ui: &mut egui::Ui, size: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let frame = ui.ctx().cumulative_pass_nr();
    let id = response.id.with("loading-bunny");
    let state = ui.ctx().data_mut(|data| {
        let previous = data.get_temp::<LoadingBunny>(id);
        let state = loading_choice(previous, frame, random_bunny);
        data.insert_temp(id, state);
        state
    });
    if ui.is_rect_visible(rect) {
        paint_loading(
            ui.painter(),
            rect.center(),
            size,
            ui.visuals().widgets.noninteractive.fg_stroke.color,
            ui.input(|input| input.time) as f32,
            state.variant,
        );
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
    }
    response
}

#[cfg(test)]
mod icon_tests {
    use super::*;

    fn painted_icons(paint: impl Fn(&egui::Painter)) -> Vec<egui::epaint::ClippedShape> {
        let ctx = egui::Context::default();
        ctx.run(egui::RawInput::default(), |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("icon-test"),
            ));
            paint(&painter);
        })
        .shapes
    }

    #[test]
    fn home_bounces_as_one_shape_and_returns_to_rest() {
        let home = |progress| {
            painted_icons(|painter| {
                page_animated(
                    painter,
                    Page::Home,
                    egui::Pos2::ZERO,
                    24.0,
                    egui::Color32::WHITE,
                    progress,
                );
            })
        };
        let rest = home(0.0);
        let moving = home(0.5);
        let finished = home(1.0);
        assert_eq!(rest.len(), 3);
        assert_eq!(moving.len(), rest.len());
        assert_eq!(finished.len(), rest.len());
        for ((rest, moving), finished) in rest.iter().zip(&moving).zip(&finished) {
            let (egui::Shape::Path(rest), egui::Shape::Path(moving), egui::Shape::Path(finished)) =
                (&rest.shape, &moving.shape, &finished.shape)
            else {
                panic!("contours de la maison attendus");
            };
            assert_eq!(rest.points.len(), moving.points.len());
            assert_eq!(rest.points.len(), finished.points.len());
            for ((rest, moving), finished) in
                rest.points.iter().zip(&moving.points).zip(&finished.points)
            {
                assert!(moving.distance(*rest + egui::vec2(0.0, -2.0)) < 0.001);
                assert!(finished.distance(*rest) < 0.001);
            }
        }
    }

    #[test]
    fn controller_has_roomier_vertical_proportions() {
        let shapes = painted_icons(|painter| {
            page_animated(
                painter,
                Page::Controllers,
                egui::Pos2::ZERO,
                24.0,
                egui::Color32::WHITE,
                0.0,
            );
        });
        let bounds = shapes.iter().fold(egui::Rect::NOTHING, |bounds, shape| {
            bounds.union(shape.shape.visual_bounding_rect())
        });
        assert!(
            bounds.height() >= 14.0,
            "manette trop aplatie : {}",
            bounds.height()
        );
    }

    #[test]
    fn appearance_brush_and_controller_motion_return_to_rest() {
        let appearance = |progress| {
            painted_icons(|painter| {
                settings_tab_animated(
                    painter,
                    SettingsTab::Appearance,
                    egui::Pos2::ZERO,
                    24.0,
                    egui::Color32::WHITE,
                    progress,
                );
            })
        };
        let rest = appearance(0.0);
        assert!(appearance(0.5).len() > rest.len());
        assert_eq!(appearance(1.0).len(), rest.len());

        let controller = |progress| {
            painted_icons(|painter| {
                page_animated(
                    painter,
                    Page::Controllers,
                    egui::Pos2::ZERO,
                    24.0,
                    egui::Color32::WHITE,
                    progress,
                );
            })
        };
        let rest = controller(0.0);
        let moving = controller(0.5);
        let finished = controller(1.0);
        assert_eq!(moving.len(), rest.len() + 2);
        assert_eq!(finished.len(), rest.len());
        let first_point = |shapes: &[egui::epaint::ClippedShape]| match &shapes[0].shape {
            egui::Shape::Path(path) => path.points[0],
            _ => panic!("contour de la manette attendu"),
        };
        assert!(first_point(&moving).distance(first_point(&rest)) > 0.5);
        assert!(first_point(&finished).distance(first_point(&rest)) < 0.01);
    }

    #[test]
    fn application_moves_both_ears_and_returns_to_rest() {
        let application = |progress| {
            painted_icons(|painter| {
                settings_tab_animated(
                    painter,
                    SettingsTab::Application,
                    egui::Pos2::ZERO,
                    24.0,
                    egui::Color32::WHITE,
                    progress,
                );
            })
        };
        let rest = application(0.0);
        let animated = application(0.5);
        let finished = application(1.0);
        let point = |shapes: &[egui::epaint::ClippedShape], index: usize| match &shapes[index].shape
        {
            egui::Shape::Path(path) => path.points[8],
            _ => panic!("courbe d'oreille attendue"),
        };
        for index in [0, 2] {
            assert!(point(&animated, index).distance(point(&rest, index)) > 0.25);
            assert!(point(&finished, index).distance(point(&rest, index)) < 0.01);
            let bounds = animated[index].shape.visual_bounding_rect();
            assert!(
                bounds.left() >= -8.5 && bounds.right() <= 8.5,
                "l'oreille dépasse la tête : {bounds:?}"
            );
        }
        for shapes in [&rest, &animated, &finished] {
            for (ear, endpoint, head) in [(0, 0, 8), (1, 16, 8), (2, 0, 7), (3, 16, 7)] {
                let (egui::Shape::Path(ear_path), egui::Shape::Path(head_path)) =
                    (&shapes[ear].shape, &shapes[head].shape)
                else {
                    panic!("courbes du lapin attendues");
                };
                let p = ear_path.points[endpoint];
                let distance = head_path
                    .points
                    .windows(2)
                    .map(|segment| {
                        let line = segment[1] - segment[0];
                        let t = ((p - segment[0]).dot(line) / line.length_sq()).clamp(0.0, 1.0);
                        p.distance(segment[0] + line * t)
                    })
                    .fold(f32::INFINITY, f32::min);
                assert!(
                    distance < 0.05,
                    "l'oreille ne rejoint pas le contour de la tête : {distance}"
                );
            }
        }
    }

    #[test]
    fn store_door_and_notification_wave_return_to_rest() {
        let store = |progress| {
            painted_icons(|painter| {
                page_animated(
                    painter,
                    Page::Boutiques,
                    egui::Pos2::ZERO,
                    24.0,
                    egui::Color32::WHITE,
                    progress,
                );
            })
        };
        let notification = |progress| {
            painted_icons(|painter| {
                notifications(
                    painter,
                    egui::Pos2::ZERO,
                    24.0,
                    egui::Color32::WHITE,
                    progress,
                );
            })
        };
        let icons: [fn(f32) -> Vec<egui::epaint::ClippedShape>; 2] = [store, notification];
        for icon in icons {
            let rest = icon(0.0);
            let animated = icon(0.5);
            let finished = icon(1.0);
            assert!(animated.len() > rest.len());
            assert_eq!(finished.len(), rest.len());
        }
    }
}

#[cfg(test)]
mod loading_tests {
    use super::*;

    #[test]
    fn all_loading_variants_paint_with_the_requested_alpha() {
        for variant in 0..LOADING_BUNNY_VARIANTS {
            let ctx = egui::Context::default();
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    egui::Id::new("loading-test"),
                ));
                paint_loading(
                    &painter,
                    egui::pos2(50.0, 50.0),
                    64.0,
                    egui::Color32::from_white_alpha(90),
                    0.35,
                    variant,
                );
            });
            assert!(output.shapes.len() >= 10, "variant {variant}");
            for shape in &output.shapes {
                let alpha = match &shape.shape {
                    egui::Shape::Path(path) => match path.stroke.color {
                        egui::epaint::ColorMode::Solid(color) => color.a(),
                        _ => panic!("unexpected loading stroke"),
                    },
                    egui::Shape::Circle(circle) => circle.fill.a(),
                    other => panic!("unexpected shape: {other:?}"),
                };
                assert!(alpha <= 90, "variant {variant}, alpha {alpha}");
            }
        }
    }

    #[test]
    fn choice_stays_stable_until_the_indicator_disappears() {
        let first = loading_choice(None, 10, || 1);
        let same_frame = loading_choice(Some(first), 10, || {
            panic!("nouveau tirage sur la même image")
        });
        let next_frame = loading_choice(Some(same_frame), 11, || {
            panic!("nouveau tirage pendant le chargement")
        });
        assert_eq!(next_frame.variant, 1);
        let new_loading = loading_choice(Some(next_frame), 13, || 4);
        assert_eq!(new_loading.variant, 4);
        assert_eq!(loading_choice(None, 20, || 5).variant, 0);
    }
}
