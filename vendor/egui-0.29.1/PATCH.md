# egui 0.29.1 — correctif hit-test

Copie locale de `egui` 0.29.1 pour Lapinux.

Les changements sont dans `src/hit_test.rs` : les widgets dont `rect` ou
`interact_rect` contient un NaN sont ignorés, et la comparaison des deux
widgets touchés ne fait plus `unwrap()`. Sans ces gardes, l'égalité d'un
`WidgetRect` contenant un NaN est toujours fausse : `position(..)` renvoie
`None` et tout le processus est tué via `panic = "abort"`.

Correctif amont : <https://github.com/emilk/egui/pull/6851>, publié dans egui
0.32.0. Cette copie conserve l'API 0.29 utilisée par l'interface.
