//! État du quick menu overlay, indépendant du rendu et testable.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Resume,
    FpsCap,
    MangoHud,
    Profile,
    Proton,
    Record,
    Screenshot,
    QuitGame,
}

pub const ITEMS: [Item; 8] = [
    Item::Resume,
    Item::FpsCap,
    Item::MangoHud,
    Item::Profile,
    Item::Proton,
    Item::Record,
    Item::Screenshot,
    Item::QuitGame,
];

impl Action {
    /// Forme envoyée au daemon par l'overlay.
    pub fn wire(&self) -> String {
        match self {
            Action::Resume => "resume",
            Action::SetFpsCap(value) => return format!("fps_cap:{value}"),
            Action::ToggleMangoHud => "mangohud_toggle",
            Action::CycleProfile => "profile_next",
            Action::CycleProton => "proton_next",
            Action::ToggleRecording => "record_toggle",
            Action::Screenshot => "screenshot",
            Action::QuitGame => "quit_game",
        }
        .to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Resume,
    SetFpsCap(u32),
    ToggleMangoHud,
    CycleProfile,
    CycleProton,
    ToggleRecording,
    Screenshot,
    QuitGame,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MenuState {
    pub game_name: String,
    pub proton: String,
    pub profile: String,
    pub fps_cap: u32,
    pub mangohud: bool,
    pub recording: bool,
    pub recorder_available: bool,
    pub selected: usize,
    /// Progression d'ouverture 0..1 (animation).
    pub anim: f32,
    /// Impulsion de sélection 0..1 (animation courte).
    pub pulse: f32,
    pub exit: bool,
}

impl MenuState {
    pub fn new(game_name: impl Into<String>) -> Self {
        Self {
            game_name: game_name.into(),
            proton: "auto".into(),
            profile: "defaut".into(),
            fps_cap: 60,
            mangohud: false,
            recording: false,
            recorder_available: false,
            selected: 0,
            anim: 0.0,
            pulse: 0.0,
            exit: false,
        }
    }

    pub fn items(&self) -> &'static [Item] {
        &ITEMS
    }

    pub fn current(&self) -> Item {
        ITEMS[self.selected.min(ITEMS.len() - 1)]
    }

    pub fn next(&mut self) {
        self.selected = (self.selected + 1) % ITEMS.len();
        self.pulse = 1.0;
    }

    pub fn prev(&mut self) {
        self.selected = (self.selected + ITEMS.len() - 1) % ITEMS.len();
        self.pulse = 1.0;
    }

    /// Ajuste la valeur de la ligne courante (gauche/droite).
    pub fn adjust(&mut self, delta: i32) -> Option<Action> {
        match self.current() {
            Item::FpsCap => {
                let steps = [30, 45, 60, 90, 120, 144, 165, 240];
                let index = steps
                    .iter()
                    .position(|v| *v >= self.fps_cap)
                    .unwrap_or(steps.len() - 1) as i32;
                let next = (index + delta).clamp(0, steps.len() as i32 - 1) as usize;
                self.fps_cap = steps[next];
                self.pulse = 1.0;
                Some(Action::SetFpsCap(self.fps_cap))
            }
            Item::MangoHud => {
                self.mangohud = !self.mangohud;
                Some(Action::ToggleMangoHud)
            }
            Item::Profile => Some(Action::CycleProfile),
            Item::Proton => Some(Action::CycleProton),
            _ => None,
        }
    }

    /// Valide la ligne courante (Entrée / A).
    pub fn activate(&mut self) -> Option<Action> {
        match self.current() {
            Item::Resume => {
                self.exit = true;
                Some(Action::Resume)
            }
            Item::FpsCap => self.adjust(1),
            Item::MangoHud => {
                self.mangohud = !self.mangohud;
                Some(Action::ToggleMangoHud)
            }
            Item::Profile => Some(Action::CycleProfile),
            Item::Proton => Some(Action::CycleProton),
            Item::Record => {
                if !self.recorder_available {
                    return None;
                }
                self.recording = !self.recording;
                Some(Action::ToggleRecording)
            }
            Item::Screenshot => Some(Action::Screenshot),
            Item::QuitGame => {
                self.exit = true;
                Some(Action::QuitGame)
            }
        }
    }

    pub fn close(&mut self) {
        self.exit = true;
    }

    /// Avance les animations ; renvoie true si un repaint est nécessaire.
    pub fn tick(&mut self, dt: f32) -> bool {
        let mut repaint = false;
        if self.anim < 1.0 {
            self.anim = (self.anim + dt / 0.14).min(1.0);
            repaint = true;
        }
        if self.pulse > 0.0 {
            self.pulse = (self.pulse - dt / 0.16).max(0.0);
            repaint = true;
        }
        repaint
    }

    pub fn label(&self, item: Item) -> String {
        match item {
            Item::Resume => "Reprendre".into(),
            Item::FpsCap => format!("Limite FPS : {}", self.fps_cap),
            Item::MangoHud => format!(
                "MangoHud : {}",
                if self.mangohud { "activé" } else { "désactivé" }
            ),
            Item::Profile => format!("Profil manettes : {}", self.profile),
            Item::Proton => format!("Proton : {}", self.proton),
            Item::Record => {
                if !self.recorder_available {
                    "Enregistrement : gpu-screen-recorder absent".into()
                } else if self.recording {
                    "Enregistrement : en cours".into()
                } else {
                    "Enregistrement : arrêté".into()
                }
            }
            Item::Screenshot => "Capture d'écran".into(),
            Item::QuitGame => format!("Quitter {}", self.game_name),
        }
    }

    pub fn hint(&self, item: Item) -> &'static str {
        match item {
            Item::Resume => "Entrée",
            Item::FpsCap => "← →",
            Item::MangoHud => "← →",
            Item::Profile => "← →",
            Item::Proton => "← → (prochain lancement)",
            Item::Record => "Entrée",
            Item::Screenshot => "Entrée",
            Item::QuitGame => "Entrée",
        }
    }

    pub fn enabled(&self, item: Item) -> bool {
        !(item == Item::Record && !self.recorder_available)
    }

    /// Rectangle de la ligne `index` dans un panneau de largeur `width`.
    pub fn row_y(&self, index: usize) -> f32 {
        74.0 + index as f32 * 44.0
    }

    pub fn panel_height(&self) -> f32 {
        74.0 + ITEMS.len() as f32 * 44.0 + 48.0
    }

    pub fn hit_row(&self, y: f32) -> Option<usize> {
        for index in 0..ITEMS.len() {
            let top = self.row_y(index);
            if y >= top && y < top + 44.0 {
                return Some(index);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_wraps() {
        let mut menu = MenuState::new("Jeu");
        menu.prev();
        assert_eq!(menu.selected, ITEMS.len() - 1);
        menu.next();
        assert_eq!(menu.selected, 0);
        assert_eq!(menu.current(), Item::Resume);
    }

    #[test]
    fn fps_cap_steps() {
        let mut menu = MenuState::new("Jeu");
        menu.selected = 1;
        assert_eq!(menu.adjust(1), Some(Action::SetFpsCap(90)));
        assert_eq!(menu.adjust(-1), Some(Action::SetFpsCap(60)));
        assert_eq!(menu.adjust(-5), Some(Action::SetFpsCap(30)));
    }

    #[test]
    fn activate_resume_exits() {
        let mut menu = MenuState::new("Jeu");
        assert_eq!(menu.activate(), Some(Action::Resume));
        assert!(menu.exit);
    }

    #[test]
    fn quit_game_exits() {
        let mut menu = MenuState::new("Jeu");
        menu.selected = ITEMS.len() - 1;
        assert_eq!(menu.activate(), Some(Action::QuitGame));
        assert!(menu.exit);
    }

    #[test]
    fn record_disabled_without_recorder() {
        let mut menu = MenuState::new("Jeu");
        menu.selected = 5;
        assert_eq!(menu.activate(), None);
        assert!(!menu.recording);
        menu.recorder_available = true;
        assert_eq!(menu.activate(), Some(Action::ToggleRecording));
        assert!(menu.recording);
        assert!(menu.label(Item::Record).contains("en cours"));
    }

    #[test]
    fn animation_progresses() {
        let mut menu = MenuState::new("Jeu");
        assert!(menu.tick(0.05));
        assert!(menu.anim > 0.0);
        menu.anim = 1.0;
        menu.pulse = 0.0;
        assert!(!menu.tick(0.05));
    }

    #[test]
    fn labels_and_hints_are_stable() {
        let mut menu = MenuState::new("Team Fortress 2");
        menu.fps_cap = 120;
        assert_eq!(menu.label(Item::FpsCap), "Limite FPS : 120");
        assert!(menu.label(Item::QuitGame).contains("Team Fortress 2"));
        assert_eq!(menu.hint(Item::Proton), "← → (prochain lancement)");
        assert!(!menu.enabled(Item::Record));
    }

    #[test]
    fn hit_testing_finds_rows() {
        let menu = MenuState::new("Jeu");
        let first = menu.row_y(0) + 10.0;
        assert_eq!(menu.hit_row(first), Some(0));
        let second = menu.row_y(1) + 10.0;
        assert_eq!(menu.hit_row(second), Some(1));
        assert_eq!(menu.hit_row(0.0), None);
    }
}
