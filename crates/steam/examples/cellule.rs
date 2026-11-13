//! Sonde : cellule CM assignée au logon.
//!
//! `steamroom` consomme la réponse de logon sans exposer `cell_id` (champ 7),
//! alors qu'un vrai client s'en sert comme `cell_id` dans
//! `CMsgClientMMSCreateLobby`. Cette sonde refait le logon par jeton avec les
//! mêmes étapes publiques et lit la réponse.
//!
//! `cargo run -p lapinux-steam --example cellule --features probes`

use lapinux_steam::session;
use prost::Message;
use steamroom::client::msg::ClientMsg;
use steamroom::client::SteamClient;
use steamroom::client::PROTOCOL_VERSION;
use steamroom::connection::CmServer;
use steamroom::connection::Protocol;
use steamroom::generated::CMsgClientLogon;
use steamroom::messages::EMsg;
use steamroom::transport::tcp::TcpTransport;
use steamroom::transport::websocket::WebSocketTransport;
use steamroom::types::SteamId;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    lapinux_steam::install_crypto_provider();
    let saved = session::load().ok_or("aucune session Steam enregistrée")?;
    let servers = CmServer::fetch()
        .await
        .unwrap_or_else(|_| CmServer::defaults());
    let mut last = String::new();
    for protocol in [Protocol::Tcp, Protocol::WebSocket] {
        let Some(server) = servers.iter().find(|server| server.protocol == protocol) else {
            continue;
        };
        let ready = match protocol {
            Protocol::Tcp => {
                let transport = TcpTransport::connect(server).await?;
                let (client, _rx) = SteamClient::connect(transport).await?;
                client.encrypt().await?.prepare().await?
            }
            Protocol::WebSocket => {
                let transport = WebSocketTransport::connect(server).await?;
                let (client, _rx) = SteamClient::connect_ws(transport).await?;
                client.prepare().await?
            }
        };
        let logon = CMsgClientLogon {
            protocol_version: Some(PROTOCOL_VERSION),
            cell_id: Some(0),
            client_os_type: Some(20),
            account_name: Some(saved.account_name.clone()),
            access_token: Some(saved.refresh_token.clone()),
            ..Default::default()
        };
        let body = logon.encode_to_vec();
        let mut msg = ClientMsg::with_body(EMsg::CLIENT_LOGON, &body);
        msg.header.steamid = Some(SteamId::from_parts(1, 1, 1, 0).raw());
        msg.header.client_sessionid = Some(0);
        match ready.login(msg).await {
            Ok((_client, response)) => {
                match parse_cell(&response.body) {
                    Some(cell) => println!("cell={cell}"),
                    None => println!("cell=absente"),
                }
                return Ok(());
            }
            Err(error) => last = format!("{error}"),
        }
    }
    Err(format!("logon impossible : {last}").into())
}

fn varint(body: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let byte = *body.get(*at)?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
}

fn skip(body: &[u8], at: &mut usize, wire: u8) -> Option<()> {
    match wire {
        0 => {
            varint(body, at)?;
        }
        1 => *at = at.checked_add(8)?,
        2 => {
            let len = varint(body, at)? as usize;
            *at = at.checked_add(len)?;
        }
        5 => *at = at.checked_add(4)?,
        _ => return None,
    }
    (*at <= body.len()).then_some(())
}

/// Lit `CMsgClientLogonResponse` et rend `cell_id` (champ 7).
fn parse_cell(body: &[u8]) -> Option<u32> {
    let mut at = 0;
    while at < body.len() {
        let key = varint(body, &mut at)?;
        let field = key >> 3;
        let wire = (key & 7) as u8;
        if field == 7 && wire == 0 {
            return Some(varint(body, &mut at)? as u32);
        }
        skip(body, &mut at, wire)?;
    }
    None
}
