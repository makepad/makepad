use makepad_micro_serde::{DeBin, DeJson, SerBin, SerJson};
use makepad_studio_protocol::backend_protocol::{
    ClientId, QueryId, StudioToUI, UIToStudio, UIToStudioEnvelope,
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
    let envelope = UIToStudioEnvelope {
        query_id: QueryId::new(ClientId(3), 99),
        msg: UIToStudio::LoadFileTree {
            mount: "makepad".to_string(),
        },
    };

    let bin = envelope.serialize_bin();
    let dec_bin = UIToStudioEnvelope::deserialize_bin(&bin).expect("deserialize bin");
    assert_eq!(dec_bin.query_id, envelope.query_id);

    let json = envelope.serialize_json();
    let dec_json = UIToStudioEnvelope::deserialize_json(&json).expect("deserialize json");
    assert_eq!(dec_json.query_id, envelope.query_id);
}

#[test]
fn studio_to_ui_binary_and_json_roundtrip() {
    let msg = StudioToUI::Hello {
        client_id: ClientId(7),
    };

    let bin = msg.serialize_bin();
    let dec_bin = StudioToUI::deserialize_bin(&bin).expect("deserialize bin");
    match dec_bin {
        StudioToUI::Hello { client_id } => assert_eq!(client_id, ClientId(7)),
        other => panic!("unexpected variant: {:?}", other),
    }

    let json = msg.serialize_json();
    let dec_json = StudioToUI::deserialize_json(&json).expect("deserialize json");
    match dec_json {
        StudioToUI::Hello { client_id } => assert_eq!(client_id, ClientId(7)),
        other => panic!("unexpected variant: {:?}", other),
    }
}
