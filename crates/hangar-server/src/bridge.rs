//! "Send to editor" bridge: editors connected to `GET /api/bridge/events` and
//! the deliveries sent to them.
//!
//! State is in memory, per server process: an editor is reachable through the
//! instance that holds its stream. Running several instances needs a fan-out
//! (Postgres LISTEN/NOTIFY) in front of [`BridgeHub::push`].

use chrono::Utc;
use hangar_core::{BridgeClientDto, BridgeImport, DeliveryDto, DeliveryState, DeliveryUpdate};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use summer_web::axum::response::sse::Event;
use tokio::sync::mpsc;
use uuid::Uuid;

/// Deliveries are kept this long after their last update, for the web to read.
const DELIVERY_TTL: chrono::Duration = chrono::Duration::hours(1);

struct Client {
    user_id: Uuid,
    dto: BridgeClientDto,
    tx: mpsc::Sender<Event>,
}

struct Delivery {
    user_id: Uuid,
    dto: DeliveryDto,
}

#[derive(Default)]
struct State {
    clients: HashMap<Uuid, Client>,
    deliveries: HashMap<Uuid, Delivery>,
}

#[derive(Clone, Default)]
pub struct BridgeHub {
    state: Arc<Mutex<State>>,
}

/// Removes its client from the hub when the events stream is dropped.
pub struct Registration {
    hub: BridgeHub,
    pub id: Uuid,
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(c) = self.hub.state.lock().unwrap().clients.remove(&self.id) {
            tracing::info!(client = %c.dto.name, app = %c.dto.app, "editor disconnected");
        }
    }
}

impl BridgeHub {
    pub fn connect(&self, user_id: Uuid, dto: BridgeClientDto, tx: mpsc::Sender<Event>) -> Registration {
        let id = dto.id;
        tracing::info!(client = %dto.name, app = %dto.app, "editor connected");
        self.state.lock().unwrap().clients.insert(id, Client { user_id, dto, tx });
        Registration { hub: self.clone(), id }
    }

    pub fn clients(&self, user_id: Uuid) -> Vec<BridgeClientDto> {
        let state = self.state.lock().unwrap();
        let mut list: Vec<BridgeClientDto> =
            state.clients.values().filter(|c| c.user_id == user_id).map(|c| c.dto.clone()).collect();
        list.sort_by_key(|c| c.connected_at);
        list
    }

    pub fn client(&self, user_id: Uuid, id: Uuid) -> Option<BridgeClientDto> {
        let state = self.state.lock().unwrap();
        state.clients.get(&id).filter(|c| c.user_id == user_id).map(|c| c.dto.clone())
    }

    /// Records a delivery and pushes it to the client. False when the client is gone
    /// or its stream is backed up.
    pub fn push(&self, user_id: Uuid, client_id: Uuid, import: &BridgeImport) -> Option<DeliveryDto> {
        let event = Event::default().event("import").json_data(import).ok()?;
        let mut state = self.state.lock().unwrap();
        let client = state.clients.get(&client_id).filter(|c| c.user_id == user_id)?;
        client.tx.try_send(event).ok()?;
        let dto = DeliveryDto {
            id: import.delivery_id,
            client_id,
            client_name: client.dto.name.clone(),
            asset_id: import.asset_id,
            version_id: import.version_id,
            version_number: import.version_number,
            state: DeliveryState::Sent,
            progress: None,
            message: None,
            imported: vec![],
            updated_at: Utc::now(),
        };
        let cutoff = Utc::now() - DELIVERY_TTL;
        state.deliveries.retain(|_, d| d.dto.updated_at > cutoff);
        state.deliveries.insert(dto.id, Delivery { user_id, dto: dto.clone() });
        Some(dto)
    }

    pub fn delivery(&self, user_id: Uuid, id: Uuid) -> Option<DeliveryDto> {
        let state = self.state.lock().unwrap();
        state.deliveries.get(&id).filter(|d| d.user_id == user_id).map(|d| d.dto.clone())
    }

    /// The editor reports progress. A final state can't be changed any more.
    pub fn update(&self, user_id: Uuid, id: Uuid, update: DeliveryUpdate) -> Option<DeliveryDto> {
        let mut state = self.state.lock().unwrap();
        let d = state.deliveries.get_mut(&id).filter(|d| d.user_id == user_id)?;
        if !d.dto.state.is_final() {
            d.dto.state = update.state;
            d.dto.progress = update.progress.map(|p| p.clamp(0.0, 1.0));
            d.dto.message = update.message.map(|m| m.chars().take(2000).collect());
            d.dto.imported = update.imported.into_iter().take(500).collect();
            d.dto.updated_at = Utc::now();
        }
        Some(d.dto.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hangar_core::AssetKind;

    fn client(id: Uuid) -> BridgeClientDto {
        BridgeClientDto { id, app: "unreal".into(), name: "PC".into(), detail: None, connected_at: Utc::now() }
    }

    fn import() -> BridgeImport {
        BridgeImport {
            delivery_id: Uuid::now_v7(),
            project_id: Uuid::nil(),
            project_slug: "p".into(),
            project_name: "P".into(),
            asset_id: Uuid::nil(),
            asset_slug: "a".into(),
            asset_name: "A".into(),
            kind: AssetKind::Model,
            version_id: Uuid::nil(),
            version_number: 1,
            sent_by: "Eu".into(),
        }
    }

    #[test]
    fn deliveries_reach_only_their_user() {
        let hub = BridgeHub::default();
        let (me, other, cid) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let (tx, mut rx) = mpsc::channel(4);
        let reg = hub.connect(me, client(cid), tx);
        assert_eq!(hub.clients(me).len(), 1);
        assert!(hub.clients(other).is_empty());
        assert!(hub.push(other, cid, &import()).is_none(), "another user can't use my editor");

        let d = hub.push(me, cid, &import()).unwrap();
        assert!(rx.try_recv().is_ok());
        assert!(hub.delivery(other, d.id).is_none());
        let done = DeliveryUpdate { state: DeliveryState::Done, progress: None, message: None, imported: vec![] };
        assert_eq!(hub.update(me, d.id, done).unwrap().state, DeliveryState::Done);
        let late = DeliveryUpdate { state: DeliveryState::Importing, progress: Some(2.0), message: None, imported: vec![] };
        assert_eq!(hub.update(me, d.id, late).unwrap().state, DeliveryState::Done, "final states stick");

        drop(reg);
        assert!(hub.clients(me).is_empty(), "a closed stream unregisters its editor");
        assert!(hub.push(me, cid, &import()).is_none());
    }
}
