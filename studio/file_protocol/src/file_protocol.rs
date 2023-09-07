use {
    crate::{
        makepad_live_id::*,
        makepad_micro_serde::{SerBin, DeBin, DeBinErr},
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
pub enum FileRequest {
    /// Requests the collab server to return its file tree. 
    LoadFileTree{ with_data: bool },
    /// Requests the collab server to add the client as a participant to the file with the given id.
    /// If the client is the first participant for the file, this also causes the file to be opened
    /// on the server.
    OpenFile(String),
    /// Requests the collab server to apply the given delta to the given revision of the file with
    /// the given id.
    SaveFile(String, String),

}

/// A type for representing either a response or a notification from the collab server.
#[derive(Clone, Debug, SerBin, DeBin)]
pub enum FileClientAction {
    Response(FileResponse),
    Notification(FileNotification),
}

/// A type for representing a response from the collab server.
/// 
/// Each `Response` corresponds to the `Request` with the same name.
#[derive(Clone, Debug, SerBin, DeBin)]
pub enum FileResponse {
    /// The result of requesting the collab server to return its file tree.
    LoadFileTree(Result<FileTreeData, FileError>),
    /// The result of requesting the collab server to add the client as a participant to the file
    /// with the given id.
    OpenFile(Result<(String, String), FileError>),
    /// The result of requesting the collab server to apply a delta to a revision of the file with
    /// the given id.
    SaveFile(Result<(String,String,String), FileError>),
}

/// A type for representing data about a file tree.
#[derive(Clone, Debug, SerBin, DeBin)]
pub struct FileTreeData {
    /// The path to the root of this file tree.
    pub root_path: String,
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
    pub name: String,
    /// The node for this entry.
    pub node: FileNodeData,
}

/// A type for representing a notification from the collab server.
#[derive(Clone, Debug, SerBin, DeBin)]
pub enum FileNotification {
    /// Notifies the client that another client applied the given delta to the file with the given
    /// id. This is only sent for files for which the client is a participant.
    DeltaWasApplied(TextFileId),
}

/// A type for representing errors from the collab server.
#[derive(Clone, Debug, SerBin, DeBin)]
pub enum FileError {
    Unknown(String),
    CannotOpen(String)
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
