
// ok so everyone sends interaction events to the system
// and the system has physics 'state' as output. 
// so how do we do this
use{
    std::sync::{Mutex, Arc},
    makepad_platform::{
        Cx,
        LiveId,
        XrState,
        Pose
    },
};

pub enum NetEvent{
    CreateObject{
        object: NetObject,
    },
    DestroyObject{
        id: LiveId,
    },
    MoveObject{
        id: LiveId,
        pose: Pose
    }
}

pub struct NetSnapshot{
    tick: u64,
    data: Vec<u8>
}

pub struct NetXrState{
    user_id: LiveId,
    user: XrState
}

pub struct NetEnvelope{
    from: LiveId,
    message: NetMessage,
}

pub enum NetMessage{
    InEvent(NetInEvent),
    OutEvent(NetOutEvent),
    Snapshot(NetSnapshot),
    XrState(NetXrState),
}

pub struct NetInEvent{
    event: NetEvent,
    tick: u64
}

pub struct NetOutEvent{
    event: NetEvent,
    tick: u64
}

pub struct NetObject{
    id: LiveId,
    shape: NetShape,
    pose: Pose
}

pub enum NetShape{
    Cube{size: f32}
}

pub struct NetPhysicsEngine{
}

pub struct NetConnection{
}

pub struct NetNodeWorker{
    is_server: bool,
    objects: Vec<NetObject>,
    engine: NetPhysicsEngine,
    tick: u64,
    // it has a list of connections to all clients
    // but it only sends its events to the master
    connections: Vec<NetConnection>,
}

pub struct NetNode{
    worker: Arc<Mutex<NetNodeWorker>>,
}

impl NetNode{
    pub fn start(cx:&mut Cx){
        
    }
}
