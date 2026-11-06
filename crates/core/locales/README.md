# Traductions de l'interface

Chaque langue de l'interface est un **catalogue TOML** de ce dossier, embarqué
dans le binaire par `include_str!`. Les libellés ne sont plus écrits en double
dans le code : une clé stable (`library.empty_title`) désigne un texte, et le
catalogue de la langue active fournit la traduction.

- `fr.toml` est la **langue source**. C'est vers elle que retombe une clé
  absente d'un autre catalogue.
- `en.toml` est la traduction anglaise.

## Ajouter un libellé

1. Choisir une clé stable, en minuscules, avec des points par section :
   `library.filter.missing`, `settings.desc.storage`.
2. Ajouter la clé dans **tous** les catalogues, avec le même texte source et sa
   traduction.
3. Si le texte contient une valeur dynamique, utiliser un placeholder nommé :
   `missing = "À installer · {missing}"`. Le nom du placeholder doit être
   identique dans toutes les langues.
4. Appeler `lapinux_core::i18n::t("library.filter.missing")`, ou
   `t_args("library.filter.missing", &[("missing", &count.to_string())])` quand
   il y a des valeurs.

## Ajouter une langue

1. Déposer `xx.toml` ici, avec toutes les clés de `fr.toml`.
2. Inscrire la langue dans `LANGUAGES` (libellé affiché dans les réglages) et
   dans `SOURCES` (`crates/core/src/i18n.rs`).
3. Lancer `cargo test -p lapinux-core i18n` : les tests refusent une clé
   manquante, un placeholder divergent ou un libellé vide.

La langue du bureau (`LANG`, `LC_MESSAGES`, `LC_ALL`) choisit la langue au
premier démarrage ; l'anglais sert de repli universel pour une locale non gérée.

## Garanties vérifiées par les tests

- **Parité** : chaque catalogue a exactement les mêmes clés que `fr.toml`.
- **Placeholders** : `{nom}` est identique dans toutes les langues.
- **Libellés non vides**.
- **Repli** : une clé absente d'une langue rend le texte français, jamais la clé.
- **Interpolation** : `t_args` remplace tous les placeholders fournis.

Ces tests tournent avec `cargo test -p lapinux-core`. Une traduction oubliée
fait donc échouer la suite, au lieu de laisser un écran en français.
