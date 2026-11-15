//! Succès des boutiques hors Steam : une forme commune, remplie par Epic
//! (GraphQL de la boutique) et GOG (`gameplay.gog.com`).
//!
//! Aucune des deux ne publie exactement les mêmes champs : Epic donne un nom
//! débloqué et un nom verrouillé, une rareté mondiale et une date ISO ; GOG
//! donne un nom unique, deux icônes et une date. La fiche du jeu n'a qu'un type
//! à connaître.

use serde::{Deserialize, Serialize};

/// Un succès tel qu'une boutique le publie.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Achievement {
    /// Identifiant stable chez la boutique.
    pub key: String,
    pub name: String,
    pub description: String,
    /// Icône du succès débloqué, ou sa version verrouillée à défaut.
    pub icon: String,
    /// Succès caché : son intitulé ne se dévoile qu'une fois obtenu.
    pub hidden: bool,
    pub unlocked: bool,
    /// Date de déblocage, en secondes Unix.
    pub unlock_time: Option<i64>,
    /// Part des joueurs qui l'ont obtenu, quand la boutique la publie.
    pub percent: Option<f32>,
}

impl Achievement {
    /// Nom à afficher : une boutique qui cache un succès non obtenu ne doit pas
    /// le nommer, la fiche affiche alors « Succès secret ».
    pub fn display_name(&self) -> &str {
        if self.hidden && !self.unlocked {
            "Succès secret"
        } else if self.name.trim().is_empty() {
            &self.key
        } else {
            &self.name
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hidden_achievement_keeps_its_name_until_it_is_won() {
        let mut achievement = Achievement {
            key: "ACH_WIN".into(),
            name: "Finir le jeu".into(),
            hidden: true,
            ..Default::default()
        };
        assert_eq!(achievement.display_name(), "Succès secret");
        achievement.unlocked = true;
        assert_eq!(achievement.display_name(), "Finir le jeu");
        // Un succès sans nom se rabat sur son identifiant plutôt que sur du vide.
        let anonymous = Achievement {
            key: "ACH_42".into(),
            ..Default::default()
        };
        assert_eq!(anonymous.display_name(), "ACH_42");
    }
}
