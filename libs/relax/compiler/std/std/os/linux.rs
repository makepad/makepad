//! std::os::linux: Linux-specific extensions.

pub mod net {
    //! `SocketAddrExt`: abstract-namespace unix socket addresses (unix(7)).
    use crate::io;
    use crate::os::unix::net::SocketAddr;

    pub trait SocketAddrExt: Sized {
        fn from_abstract_name<N>(name: N) -> io::Result<Self>
        where
            N: AsRef<[u8]>;
        fn as_abstract_name(&self) -> Option<&[u8]>;
    }

    impl SocketAddrExt for SocketAddr {
        fn from_abstract_name<N>(name: N) -> io::Result<Self>
        where
            N: AsRef<[u8]>,
        {
            crate::os::unix::net::abstract_addr(name.as_ref())
        }
        fn as_abstract_name(&self) -> Option<&[u8]> {
            crate::os::unix::net::abstract_name_of(self)
        }
    }
}
