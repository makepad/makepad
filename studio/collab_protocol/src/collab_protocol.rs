use {
    crate::{
        makepad_editor_core::{
            delta::Delta,
            text::Text
        },
        makepad_live_id::*,
        makepad_micro_serde::{SerBin, DeBin, DeBinErr},
        unix_path::UnixPathBuf,
        unix_str::UnixString,
    },
};

/// Types for the collab protocol.
/// 
/// The collab protocol is relatively simple. The collab server can open and close files. Each open
/// file has a corresponding collaboration session, to/from which clients can add/remove themselves
/// as participant. When a client requests to open a file, it really requests to be added as a
/// participant to (the collaboration session of) that file. Similarly, when a client requests to
/// close a file, it really requests to be removed as a participant from (the collaboration session)
/// of that file. Files are only opened/closed as necessary, that is, when the first client is added
/// or the last client removed as a participant.
/// 
/// Once the client is a participant for a file, it can request to apply deltas to that file. Deltas
/// are always applied to a given revision of a file. Because of network latency, different clients
/// clients may have different revisions of the same file. The server maintains a linear history of
/// all deltas from the oldest revision to the newest revision. Whenever a delta for an older
/// revision comes in, it is transformed against these older revisions so it can be applied to the
/// newest revision. Only when all clients have confirmed that they have seen a revision (by sending
/// a delta based on that revision) will the server remove that delta from its history.
/// 
/// Whenever a server applies a delta to a file, it notifies all the participants of that file
/// except the one from which the request to apply the delta originated of this fact. This allows
/// the participants to update their revision of the file accordingly.
 
/// A type for representing a request to the collab server.
#[derive(Clone, Debug, SerBin, DeBin)]
pub enum CollabRequest {
    /// Requests the collab server to return its file tree. 
    LoadFileTree{ with_data: bool },
    /// Requests the collab server to add the client as a participant to the file with the given id.
    /// If the client is the first participant for the file, this also causes the file to be opened
    /// on the server.
    OpenFile(UnixPathBuf),
    /// Requests the collab server to apply the given delta to the given revision of the file with
    /// the given id.
    ApplyDelta(TextFileId, u32, Delta),
    /// Requests the collab server to remove the client as a participant from the file with the
    /// given id. If the client was the last participant for the file, this also closes the file on
    /// the collab server.
    CloseFile(TextFileId),
}

/// A type for representing either a response or a notification from the collab server.
#[derive(Clone, Debug, SerBin, DeBin)]
pub enum CollabClientAction {
    Response(CollabResponse),
    Notification(CollabNotification),
}

/// A type for representing a response from the collab server.
/// 
/// Each `Response` corresponds to the `Request` with the same name.
#[derive(Clone, Debug, SerBin, DeBin)]
pub enum CollabResponse {
    /// The result of requesting the collab server to return its file tree.
    LoadFileTree(Result<FileTreeData, CollabError>),
    /// The result of requesting the collab server to add the client as a participant to the file
    /// with the given id.
    OpenFile(Result<(TextFileId, u32, Text), CollabError>),
    /// The result of requesting the collab server to apply a delta to a revision of the file with
    /// the given id.
    ApplyDelta(Result<TextFileId, CollabError>),
    /// The result of requesting the collab server to remove the client as a participant from the
    /// file with the given id.
    CloseFile(Result<TextFileId, CollabError>),
}

/// A type for representing data about a file tree.
#[derive(Clone, Debug, SerBin, DeBin)]
pub struct FileTreeData {
    /// The path to the root of this file tree.
    pub path: UnixPathBuf,
    /// Data about the root of this file tree.
    pub root: FileNodeData,
}

/// A type for representing data about a node in a file tree.
/// 
/// Each node is either a directory a file. Directories form the internal nodes of the file tree.
/// They consist of one or more named entries, each of which is another node. Files form the leaves
/// of the file tree, and do not contain any further nodes.
#[derive(Clone, Debug, SerBin, DeBin)]
pub enum FileNodeData {
    Directory { entries: Vec<DirectoryEntry> },
    File { data: Option<Vec<u8>> },
}

/// A type for representing an entry in a directory.
#[derive(Clone, Debug, SerBin, DeBin)]
pub struct DirectoryEntry {
    /// The name of this entry.
    pub name: UnixString,
    /// The node for this entry.
    pub node: FileNodeData,
}

/// A type for representing a notification from the collab server.
#[derive(Clone, Debug, SerBin, DeBin)]
pub enum CollabNotification {
    /// Notifies the client that another client applied the given delta to the file with the given
    /// id. This is only sent for files for which the client is a participant.
    DeltaWasApplied(TextFileId, Delta),
}

/// A type for representing errors from the collab server.
#[derive(Clone, Debug, SerBin, DeBin)]
pub enum CollabError {
    /// Attempted to add the client as a participant to a file for which it was already a
    /// participant.
    AlreadyAParticipant,
    /// Attempted to either apply a delta to, or remove the client as a participant from a file for
    /// which it was not a participant.
    NotAParticipant,
    /// Unknown error
    Unknown(String),
}

/// An identifier for files on the collab server.
#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct TextFileId(pub LiveId);

impl SerBin for TextFileId {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        self.0.0.ser_bin(s);
    }
}

impl DeBin for TextFileId {
    fn de_bin(o: &mut usize, d: &[u8]) -> Result<Self, DeBinErr> {
        Ok(TextFileId(LiveId(DeBin::de_bin(o, d)?)))
    }
}
