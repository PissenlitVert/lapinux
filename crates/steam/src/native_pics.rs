//! Requêtes PICS et recherche de produits.

use super::*;

pub(super) async fn product(
    root_appid: u32,
    client: &SteamClient<LoggedIn>,
) -> Result<(AppMeta, KeyValue)> {
    let app_id = AppId(root_appid);
    let tokens = client
        .pics_get_access_tokens(&[app_id])
        .await
        .map_err(steam_error)?;
    let token = tokens
        .iter()
        .find(|token| token.app_id == app_id)
        .map(|token| token.token)
        .unwrap_or(0);
    let infos = client
        .pics_get_product_info(&[AccessToken { app_id, token }])
        .await
        .map_err(steam_error)?;
    let info: AppInfo = infos
        .into_iter()
        .next()
        .ok_or_else(|| Error::Other(format!("PICS ne connaît pas l'application {root_appid}")))?;
    let bytes = info
        .kv_data
        .ok_or_else(|| Error::Other(format!("PICS sans données pour {root_appid}")))?;
    let root = parse_kv(&bytes)
        .ok_or_else(|| Error::Other(format!("réponse PICS illisible pour {root_appid}")))?;
    let meta = meta_from_pics(&root);
    Ok((meta, root))
}

/// Identifiants demandés que la réponse n'a pas rendus.
pub(super) fn missing_ids(requested: &[u32], returned: &[u32]) -> Vec<u32> {
    requested
        .iter()
        .filter(|id| !returned.contains(id))
        .copied()
        .collect()
}

/// Lot suivant pour les identifiants que Steam n'a pas rendus : on resserre de
/// moitié, jusqu'à l'appel unitaire. `None` quand l'appel unitaire lui-même n'a
/// rien rendu : l'identifiant est alors abandonné, sans boucle sans fin.
pub(super) fn smaller_batch(batch: usize) -> Option<usize> {
    (batch > 1).then(|| (batch / 2).max(1))
}

/// Fiches de paquets, sans perte de lot.
///
/// Le CM découpe ses réponses en plusieurs messages et `steamroom` ne rend que
/// le premier : la fin d'un gros lot disparaît sans erreur. Les paquets absents
/// sont redemandés par lots plus petits, jusqu'à l'appel unitaire.
pub(super) async fn package_infos(
    client: &SteamClient<LoggedIn>,
    packages: &[u32],
) -> Result<Vec<PackageInfo>> {
    let mut wanted = packages.to_vec();
    let mut batch = PICS_BATCH;
    let mut infos = Vec::new();
    while !wanted.is_empty() {
        let mut missing = Vec::new();
        for chunk in wanted.chunks(batch) {
            let ids: Vec<PackageId> = chunk.iter().copied().map(PackageId).collect();
            let found = client
                .pics_get_package_info(&ids)
                .await
                .map_err(steam_error)?;
            let returned: Vec<u32> = found
                .iter()
                .filter_map(|info| info.package_id.map(|id| id.0))
                .collect();
            missing.extend(missing_ids(chunk, &returned));
            infos.extend(found);
        }
        if missing.len() == wanted.len() {
            match smaller_batch(batch) {
                Some(smaller) => batch = smaller,
                None => break,
            }
        }
        wanted = missing;
    }
    Ok(infos)
}

/// Fiches d'applications, sans perte de lot (même découpage que
/// [`package_infos`]).
pub(super) async fn app_infos(
    client: &SteamClient<LoggedIn>,
    tokens: &[AccessToken],
) -> Result<Vec<AppInfo>> {
    let mut wanted = tokens.to_vec();
    let mut batch = PICS_INFO_BATCH;
    let mut infos = Vec::new();
    while !wanted.is_empty() {
        let mut missing: Vec<AccessToken> = Vec::new();
        for chunk in wanted.chunks(batch) {
            let found = client
                .pics_get_product_info(chunk)
                .await
                .map_err(steam_error)?;
            let returned: Vec<u32> = found
                .iter()
                .filter_map(|info| info.app_id.map(|id| id.0))
                .collect();
            missing.extend(
                chunk
                    .iter()
                    .filter(|token| !returned.contains(&token.app_id.0))
                    .cloned(),
            );
            infos.extend(found);
        }
        if missing.len() == wanted.len() {
            match smaller_batch(batch) {
                Some(smaller) => batch = smaller,
                None => break,
            }
        }
        wanted = missing;
    }
    Ok(infos)
}
