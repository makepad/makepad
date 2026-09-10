"""Read-only Btrfs inspection for this single-device USB. Never writes a filesystem."""
import struct
import uuid
import zlib
from compression import zstd

def u16(b, o): return struct.unpack_from('<H', b, o)[0]
def u32(b, o): return struct.unpack_from('<I', b, o)[0]
def u64(b, o): return struct.unpack_from('<Q', b, o)[0]
def key(b, o=0): return (u64(b, o), b[o+8], u64(b, o+9))

_crc_table=[]
for _n in range(256):
    _c=_n
    for _ in range(8): _c=(_c>>1) ^ (0x82f63b78 if _c&1 else 0)
    _crc_table.append(_c)
def crc32c(data):
    c=0xffffffff
    for byte in data: c=_crc_table[(c ^ byte)&255] ^ (c>>8)
    return c ^ 0xffffffff

class Btrfs:
    def __init__(self, stream, start):
        self.stream=stream; self.start=start; self.chunks={}; self.nodes={}
        self.sb=self.physical(65536,4096)
        assert self.sb[64:72]==b'_BHRfS_M', 'Not Btrfs'
        assert u16(self.sb,196)==0, 'Expected CRC32C filesystem'
        assert crc32c(self.sb[32:])==u32(self.sb,0), 'Bad Btrfs superblock checksum'
        assert u64(self.sb,136)==1, 'Expected one Btrfs device'
        self.uuid=str(uuid.UUID(bytes=self.sb[32:48]))
        self.nodesize=u32(self.sb,148)
        assert self.nodesize in (4096,8192,16384,32768,65536)
        p=0x32b; end=p+u32(self.sb,160)
        while p<end:
            k=key(self.sb,p); p+=17
            assert k[1]==228
            n=48+32*u16(self.sb,p+44)
            self.chunk(k[2],self.sb[p:p+n]); p+=n
        assert p==end
        for k,data in self.items(u64(self.sb,88)):
            if k[1]==228: self.chunk(k[2],data)
        self.roots={k[0]:u64(data,176) for k,data in self.items(u64(self.sb,80)) if k[1]==132}
        self.fsroot=self.roots[5]
        self.index={}
        for k,data in self.items(self.fsroot): self.index.setdefault(k[0],[]).append((k,data))

    def physical(self,offset,length):
        assert offset>=0 and length>=0
        self.stream.seek(self.start+offset); data=self.stream.read(length)
        assert len(data)==length, 'Short Btrfs read'
        return data

    def chunk(self,logical,data):
        count=u16(data,44); flags=u64(data,24)
        # Only SINGLE or DUP profiles; no striped/address-interleaved layouts.
        assert flags & ~0x27 == 0, f'Unsupported chunk profile {flags:x}'
        assert 1<=count<=2
        self.chunks[logical]=(u64(data,0),u64(data,56),flags)

    def logical(self,offset,length):
        out=bytearray()
        while length:
            for start,(size,physical,_) in self.chunks.items():
                if start<=offset<start+size:
                    n=min(length,start+size-offset)
                    out.extend(self.physical(physical+offset-start,n))
                    offset+=n; length-=n; break
            else: raise RuntimeError(f'No chunk mapping for {offset}')
        return bytes(out)

    def items(self,address):
        if address not in self.nodes:
            b=self.logical(address,self.nodesize)
            assert u64(b,48)==address and crc32c(b[32:])==u32(b,0), 'Bad Btrfs node'
            self.nodes[address]=b
        b=self.nodes[address]; count=u32(b,96); level=b[100]
        assert count*self.nodesize>=0 and level<8
        if level:
            assert 101+count*33<=self.nodesize
            for i in range(count): yield from self.items(u64(b,101+i*33+17))
        else:
            assert 101+count*25<=self.nodesize
            for i in range(count):
                o=101+i*25; p=101+u32(b,o+17); n=u32(b,o+21)
                assert 101<=p<=p+n<=self.nodesize
                yield key(b,o), b[p:p+n]

    def directory(self,inode):
        result={}
        for k,data in self.index.get(inode,[]):
            if k[1]!=84: continue
            p=0
            while p<len(data):
                loc=key(data,p); n=u16(data,p+27); d=u16(data,p+25)
                name=data[p+30:p+30+n].decode('utf-8','surrogateescape')
                result[name]=loc
                p+=30+n+d
            assert p==len(data)
        return result

    def lookup(self,path):
        inode=256
        for part in path.strip('/').split('/'):
            if not part: continue
            entry=self.directory(inode).get(part)
            if entry is None: raise FileNotFoundError(path)
            assert entry[1]==1, 'Subvolume traversal unsupported'
            inode=entry[0]
        return inode

    def read(self,path,limit=128*1024*1024):
        inode=self.lookup(path); entries=self.index[inode]
        inode_data=next(data for k,data in entries if k[1]==1)
        size=u64(inode_data,16)
        assert size<=limit, f'File too large: {path} ({size})'
        result=bytearray(size)
        for k,data in entries:
            if k[1]!=108: continue
            compression=data[16]; extent_type=data[20]
            assert data[17:20]==b'\0\0\0'
            offset=0
            if extent_type==0:
                blob=data[21:]; length=u64(data,8)
            elif extent_type==1:
                bytenr=u64(data,21); length=u64(data,45); offset=u64(data,37)
                if not bytenr: continue
                blob=self.logical(bytenr,u64(data,29))
            elif extent_type==2: continue
            else: raise RuntimeError(f'Unknown extent type {extent_type}')
            if compression==1: blob=zlib.decompress(blob)
            elif compression==3:
                dec=zstd.ZstdDecompressor(); blob=dec.decompress(blob)
                assert dec.eof
            else: assert compression==0, f'Unsupported compression {compression}'
            count=min(length,size-k[2])
            assert count>=0 and offset+count<=len(blob)
            result[k[2]:k[2]+count]=blob[offset:offset+count]
        return bytes(result)

if __name__=='__main__':
    import argparse
    import sys
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('image')
    parser.add_argument('--offset', type=int, required=True, help='Filesystem start in bytes')
    parser.add_argument('paths', nargs='*')
    args=parser.parse_args()
    if sys.flags.optimize:
        parser.error('Run without -O; filesystem verification requires assertions')
    if args.offset<0:
        parser.error('--offset must be nonnegative')
    with open(args.image,'rb') as stream:
        fs=Btrfs(stream,args.offset)
        print('UUID',fs.uuid,'generation',u64(fs.sb,72),'root',fs.fsroot)
        print('Root entries', sorted(fs.directory(256)))
        for path in args.paths:
            print(path)
            print(fs.read(path).decode('utf-8','replace'))
