use super::{
    XrNetActivityControl, XrNetAlignmentDescriptorFrame, XrNetAlignmentFrame, XrNetBodySpawn,
    XrNetPeerId, XrNetSharedObjectControl, XrNetSharedObjectState, XrNetStateFrame,
    XR_NET_PROTOCOL_VERSION, XR_NET_SYNC_FRAME_LZ4_TAG, XR_NET_SYNC_FRAME_RAW_TAG,
    XR_NET_SYNC_LZ4_ACCELERATION, XR_NET_SYNC_MAX_FRAME_BYTES,
};
use makepad_lz4::{compress_bound, compress_fast_into, decompress_safe};
use makepad_widgets::makepad_platform::makepad_micro_serde::*;
use std::io;

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct XrNetDiscoveryHello {
    pub version: u16,
    pub node_id: XrNetPeerId,
    pub data_port: u16,
    pub sync_port: u16,
}

impl XrNetDiscoveryHello {
    pub fn is_compatible_for(&self, local_node_id: XrNetPeerId) -> bool {
        self.version == XR_NET_PROTOCOL_VERSION && self.node_id != local_node_id
    }
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct XrNetLeavePacket {
    pub version: u16,
    pub node_id: XrNetPeerId,
}

impl XrNetLeavePacket {
    pub fn is_compatible_for(&self, local_node_id: XrNetPeerId) -> bool {
        self.version == XR_NET_PROTOCOL_VERSION && self.node_id != local_node_id
    }
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum XrNetDiscoveryPacket {
    Hello(XrNetDiscoveryHello),
    Leave(XrNetLeavePacket),
}

impl XrNetDiscoveryPacket {
    pub fn hello(node_id: XrNetPeerId, data_port: u16, sync_port: u16) -> Self {
        Self::Hello(XrNetDiscoveryHello {
            version: XR_NET_PROTOCOL_VERSION,
            node_id,
            data_port,
            sync_port,
        })
    }

    pub fn leave(node_id: XrNetPeerId) -> Self {
        Self::Leave(XrNetLeavePacket {
            version: XR_NET_PROTOCOL_VERSION,
            node_id,
        })
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Self::deserialize_bin(bytes).ok()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.serialize_bin()
    }
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct XrNetSyncHello {
    pub version: u16,
    pub node_id: XrNetPeerId,
}

impl XrNetSyncHello {
    pub fn is_compatible_for(&self, local_node_id: XrNetPeerId) -> bool {
        self.version == XR_NET_PROTOCOL_VERSION && self.node_id != local_node_id
    }

    pub fn matches_peer(&self, peer_id: XrNetPeerId) -> bool {
        self.version == XR_NET_PROTOCOL_VERSION && self.node_id == peer_id
    }
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum XrNetSyncPacket {
    Hello(XrNetSyncHello),
    Data(XrNetDataPacket),
}

impl XrNetSyncPacket {
    pub fn hello(node_id: XrNetPeerId) -> Self {
        Self::Hello(XrNetSyncHello {
            version: XR_NET_PROTOCOL_VERSION,
            node_id,
        })
    }

    pub fn data(packet: XrNetDataPacket) -> Self {
        Self::Data(packet)
    }
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum XrNetDataPacket {
    State {
        version: u16,
        node_id: XrNetPeerId,
        frame: XrNetStateFrame,
    },
    Alignment {
        version: u16,
        node_id: XrNetPeerId,
        frame: XrNetAlignmentFrame,
    },
    AlignmentDescriptor {
        version: u16,
        node_id: XrNetPeerId,
        frame: XrNetAlignmentDescriptorFrame,
    },
    ActivityControl {
        version: u16,
        node_id: XrNetPeerId,
        control: XrNetActivityControl,
    },
    BodySpawn {
        version: u16,
        node_id: XrNetPeerId,
        spawn: XrNetBodySpawn,
    },
    SharedObjectState {
        version: u16,
        node_id: XrNetPeerId,
        state: XrNetSharedObjectState,
    },
    SharedObjectControl {
        version: u16,
        node_id: XrNetPeerId,
        control: XrNetSharedObjectControl,
    },
    Leave(XrNetLeavePacket),
}

impl XrNetDataPacket {
    pub fn state(node_id: XrNetPeerId, frame: XrNetStateFrame) -> Self {
        Self::State {
            version: XR_NET_PROTOCOL_VERSION,
            node_id,
            frame,
        }
    }

    pub fn alignment(node_id: XrNetPeerId, frame: XrNetAlignmentFrame) -> Self {
        Self::Alignment {
            version: XR_NET_PROTOCOL_VERSION,
            node_id,
            frame,
        }
    }

    pub fn alignment_descriptor(
        node_id: XrNetPeerId,
        frame: XrNetAlignmentDescriptorFrame,
    ) -> Self {
        Self::AlignmentDescriptor {
            version: XR_NET_PROTOCOL_VERSION,
            node_id,
            frame,
        }
    }

    pub fn activity_control(node_id: XrNetPeerId, control: XrNetActivityControl) -> Self {
        Self::ActivityControl {
            version: XR_NET_PROTOCOL_VERSION,
            node_id,
            control,
        }
    }

    pub fn body_spawn(node_id: XrNetPeerId, spawn: XrNetBodySpawn) -> Self {
        Self::BodySpawn {
            version: XR_NET_PROTOCOL_VERSION,
            node_id,
            spawn,
        }
    }

    pub fn shared_object_state(node_id: XrNetPeerId, state: XrNetSharedObjectState) -> Self {
        Self::SharedObjectState {
            version: XR_NET_PROTOCOL_VERSION,
            node_id,
            state,
        }
    }

    pub fn shared_object_control(node_id: XrNetPeerId, control: XrNetSharedObjectControl) -> Self {
        Self::SharedObjectControl {
            version: XR_NET_PROTOCOL_VERSION,
            node_id,
            control,
        }
    }

    pub fn leave(node_id: XrNetPeerId) -> Self {
        Self::Leave(XrNetLeavePacket {
            version: XR_NET_PROTOCOL_VERSION,
            node_id,
        })
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Self::deserialize_bin(bytes).ok()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.serialize_bin()
    }

    pub fn version(&self) -> u16 {
        match self {
            Self::State { version, .. }
            | Self::Alignment { version, .. }
            | Self::AlignmentDescriptor { version, .. }
            | Self::ActivityControl { version, .. }
            | Self::BodySpawn { version, .. }
            | Self::SharedObjectState { version, .. }
            | Self::SharedObjectControl { version, .. } => *version,
            Self::Leave(packet) => packet.version,
        }
    }

    pub fn sender(&self) -> XrNetPeerId {
        match self {
            Self::State { node_id, .. }
            | Self::Alignment { node_id, .. }
            | Self::AlignmentDescriptor { node_id, .. }
            | Self::ActivityControl { node_id, .. }
            | Self::BodySpawn { node_id, .. }
            | Self::SharedObjectState { node_id, .. }
            | Self::SharedObjectControl { node_id, .. } => *node_id,
            Self::Leave(packet) => packet.node_id,
        }
    }

    pub fn is_compatible_for(&self, local_node_id: XrNetPeerId) -> bool {
        self.version() == XR_NET_PROTOCOL_VERSION && self.sender() != local_node_id
    }
}

pub struct XrNetSyncFrameCodec;

impl XrNetSyncFrameCodec {
    pub fn encode(packet: &XrNetSyncPacket) -> Option<Vec<u8>> {
        let payload = packet.serialize_bin();
        let raw_frame_len = 1 + payload.len();
        if raw_frame_len > XR_NET_SYNC_MAX_FRAME_BYTES {
            return None;
        }

        let bound = compress_bound(payload.len());
        if bound != 0 {
            let mut compressed = vec![0u8; bound];
            if let Ok(compressed_len) =
                compress_fast_into(&payload, &mut compressed, XR_NET_SYNC_LZ4_ACCELERATION)
            {
                let compressed_frame_len = 1 + 4 + compressed_len;
                if compressed_frame_len < raw_frame_len
                    && compressed_frame_len <= XR_NET_SYNC_MAX_FRAME_BYTES
                {
                    let mut frame = Vec::with_capacity(compressed_frame_len);
                    frame.push(XR_NET_SYNC_FRAME_LZ4_TAG);
                    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
                    frame.extend_from_slice(&compressed[..compressed_len]);
                    return Some(frame);
                }
            }
        }

        let mut frame = Vec::with_capacity(raw_frame_len);
        frame.push(XR_NET_SYNC_FRAME_RAW_TAG);
        frame.extend_from_slice(&payload);
        Some(frame)
    }

    pub fn decode(frame: &[u8]) -> io::Result<XrNetSyncPacket> {
        let Some((&tag, payload)) = frame.split_first() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "empty sync frame",
            ));
        };

        let packet_bytes = match tag {
            XR_NET_SYNC_FRAME_RAW_TAG => payload.to_vec(),
            XR_NET_SYNC_FRAME_LZ4_TAG => {
                if payload.len() < 4 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "compressed sync frame missing decoded length",
                    ));
                }
                let decoded_len =
                    u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
                if decoded_len > XR_NET_SYNC_MAX_FRAME_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "compressed sync frame too large",
                    ));
                }
                let mut decoded = vec![0u8; decoded_len];
                let written = decompress_safe(&payload[4..], &mut decoded).map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "failed to decompress sync frame",
                    )
                })?;
                if written != decoded_len {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "compressed sync frame decoded length mismatch",
                    ));
                }
                decoded
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unknown sync frame tag",
                ))
            }
        };

        XrNetSyncPacket::deserialize_bin(&packet_bytes).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "failed to decode sync packet payload",
            )
        })
    }

    pub fn drain_packets(read_buf: &mut Vec<u8>) -> io::Result<Vec<XrNetSyncPacket>> {
        let mut packets = Vec::<XrNetSyncPacket>::new();
        let mut offset = 0usize;
        while read_buf.len().saturating_sub(offset) >= 4 {
            let frame_len = u32::from_le_bytes([
                read_buf[offset],
                read_buf[offset + 1],
                read_buf[offset + 2],
                read_buf[offset + 3],
            ]) as usize;
            if frame_len > XR_NET_SYNC_MAX_FRAME_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "sync frame exceeded max size",
                ));
            }
            if read_buf.len().saturating_sub(offset + 4) < frame_len {
                break;
            }
            let start = offset + 4;
            let end = start + frame_len;
            let packet = Self::decode(&read_buf[start..end])?;
            packets.push(packet);
            offset = end;
        }
        if offset > 0 {
            Self::consume_vec_prefix(read_buf, offset);
        }
        Ok(packets)
    }

    fn consume_vec_prefix(buf: &mut Vec<u8>, len: usize) {
        if len == 0 {
            return;
        }
        if len >= buf.len() {
            buf.clear();
            return;
        }
        let remaining = buf.len() - len;
        buf.copy_within(len.., 0);
        buf.truncate(remaining);
    }
}
