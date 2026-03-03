use makepad_micro_serde::{DeBin, DeJson, SerBin, SerJson};
use makepad_studio_protocol::hub_protocol::{
    ClientId, QueryId, HubToClient, ClientToHub, ClientToHubEnvelope,
};

#[test]
fn query_id_layout_roundtrip() {
    let client = ClientId(42);
    let qid = QueryId::new(client, 123456789);
    assert_eq!(qid.client_id(), client);
    assert_eq!(qid.counter(), 123456789);
}

#[test]
fn ui_envelope_binary_and_json_roundtrip() {
    let envelope = ClientToHubEnvelope {
        query_id: QueryId::new(ClientId(3), 99),
        msg: ClientToHub::LoadFileTree {
            mount: "makepad".to_string(),
        },
    };

    let bin = envelope.serialize_bin();
    let dec_bin = ClientToHubEnvelope::deserialize_bin(&bin).expect("deserialize bin");
    assert_eq!(dec_bin.query_id, envelope.query_id);

    let json = envelope.serialize_json();
    let dec_json = ClientToHubEnvelope::deserialize_json(&json).expect("deserialize json");
    assert_eq!(dec_json.query_id, envelope.query_id);
}

#[test]
fn studio_to_ui_binary_and_json_roundtrip() {
    let msg = HubToClient::Hello {
        client_id: ClientId(7),
    };

    let bin = msg.serialize_bin();
    let dec_bin = HubToClient::deserialize_bin(&bin).expect("deserialize bin");
    match dec_bin {
        HubToClient::Hello { client_id } => assert_eq!(client_id, ClientId(7)),
        other => panic!("unexpected variant: {:?}", other),
    }

    let json = msg.serialize_json();
    let dec_json = HubToClient::deserialize_json(&json).expect("deserialize json");
    match dec_json {
        HubToClient::Hello { client_id } => assert_eq!(client_id, ClientId(7)),
        other => panic!("unexpected variant: {:?}", other),
    }
}
