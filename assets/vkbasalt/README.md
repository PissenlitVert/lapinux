# Runtime vkBasalt embarqué

Ce dossier contient le runtime vkBasalt fourni par Lapinux pour que
l'utilisateur n'ait pas à installer un layer Vulkan système.

- Version amont : `0.3.2.10` (tag `v0.3.2.10`)
- Source : <https://github.com/DadSchoorse/vkBasalt/releases/tag/v0.3.2.10>
- Archive source : `vkBasalt-0.3.2.10.tar.gz`
- SHA-256 de l'archive source : `eb196ff446fa36ec0ca99c4406d753c1fa210afddeec5e7a76e1c2e74ed605e3`
- Licence : zlib, voir `LICENSE`
- Architectures : `x86_64` et `i686` (jeux Steam 32 bits sous Proton)
- Build : Ubuntu 20.04, GCC 10, Meson/Ninja en release, puis `strip --strip-unneeded`
- Symboles glibc les plus récents : `GLIBC_2.29` (Steam Linux Runtime 3 / Sniper et distributions récentes)
- SHA-256 `x86_64` : `b5652abc53378b6004b4b6193e72b92dd3bafb51f3903acf79e99c1a1966fa4c`
- SHA-256 `i686` : `d6421fb84aa79268abdd9289f0be0b203c32f221edac72e0332f5a63d55fb20f`

Chaque `vkBasalt.json` est un manifeste de layer implicite avec un
`library_path` relatif. Lapinux installe les deux architectures dans son
dossier de données et ajoute le dossier correspondant au lancement via
`VK_ADD_IMPLICIT_LAYER_PATH`, sans écrire dans `/usr` et sans root.

Les deux bibliothèques ont été chargées par le loader Vulkan du système avec
`ENABLE_VKBASALT=1` et le manifeste local : le layer
`VK_LAYER_VKBASALT_post_processing` est inséré et vkBasalt s'initialise.

Pour reconstruire les runtimes : `scripts/build-vkbasalt-runtime.sh`.
