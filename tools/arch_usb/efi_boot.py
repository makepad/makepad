"""Assemble the removable UEFI loader and Ethernet-only GRUB configuration."""
import hashlib
import struct

def u16(b,o): return struct.unpack_from('<H',b,o)[0]
def u32(b,o): return struct.unpack_from('<I',b,o)[0]

class Fat:
    def __init__(self,data):
        self.data=bytearray(data)
        self.bps=u16(data,11); self.spc=data[13]; self.res=u16(data,14)
        self.nfat=data[16]; self.fatsz=u32(data,36); self.root=u32(data,44)
        self.total=u32(data,32)
        assert data[510:512]==b'\x55\xaa' and self.bps in (512,1024,2048,4096)
        assert self.spc and self.spc & (self.spc-1)==0
        assert self.nfat==2 and u16(data,17)==0 and u16(data,22)==0
        assert self.total*self.bps<=len(data)<(self.total+8)*self.bps
        self.fatstart=self.res*self.bps
        self.datastart=(self.res+self.nfat*self.fatsz)*self.bps
        self.cluster_bytes=self.spc*self.bps
        self.cluster_count=(self.total*self.bps-self.datastart)//self.cluster_bytes
        assert self.cluster_count>=65525
        size=self.fatsz*self.bps
        assert data[self.fatstart:self.fatstart+size]==data[self.fatstart+size:self.fatstart+2*size]

    def fat(self,cluster):
        return u32(self.data,self.fatstart+cluster*4)&0x0fffffff

    def offset(self,cluster):
        assert 2<=cluster<self.cluster_count+2
        return self.datastart+(cluster-2)*self.cluster_bytes

    def chain(self,cluster):
        seen=set()
        while cluster<0x0ffffff8:
            assert cluster not in seen
            seen.add(cluster); yield cluster
            self.offset(cluster)
            cluster=self.fat(cluster)

    def entries(self,cluster):
        for c in self.chain(cluster):
            start=self.offset(c)
            for off in range(start,start+self.cluster_bytes,32):
                e=self.data[off:off+32]
                if e[0]==0: return
                if e[0]==0xe5 or e[11]==0x0f or e[11]&8: continue
                name=bytes(e[:8]).decode('ascii').rstrip()
                ext=bytes(e[8:11]).decode('ascii').rstrip()
                if ext: name+='.'+ext
                yield name,off,(u16(e,20)<<16)|u16(e,26),u32(e,28),bool(e[11]&16)

    def lookup(self,path):
        cluster=self.root
        for component in path.strip('/').split('/'):
            entry=next((e for e in self.entries(cluster) if e[0]==component.upper()),None)
            if entry is None: raise FileNotFoundError(path)
            cluster=entry[2]
        return entry

    def read(self,path):
        _,_,cluster,size,isdir=self.lookup(path)
        assert not isdir
        return b''.join(bytes(self.data[self.offset(c):self.offset(c)+self.cluster_bytes]) for c in self.chain(cluster))[:size]

    def replace_same_size(self,path,content):
        _,_,cluster,size,isdir=self.lookup(path)
        assert not isdir and len(content)==size
        p=0
        for c in self.chain(cluster):
            n=min(self.cluster_bytes,len(content)-p)
            if not n: break
            start=self.offset(c); self.data[start:start+n]=content[p:p+n]; p+=n
        assert p==len(content)

    def add_config(self,content):
        assert len(content)<=self.cluster_bytes
        directory=self.lookup('/EFI/BOOT')[2]
        assert not any(e[0]=='GRUB.CFG' for e in self.entries(directory)), 'Boot configuration already exists; inspect before replacing'
        slot=None
        for c in self.chain(directory):
            start=self.offset(c)
            for off in range(start,start+self.cluster_bytes-32,32):
                if self.data[off]==0 and self.data[off+32]==0:
                    slot=off; break
            if slot is not None: break
        assert slot is not None, 'No free directory slot'
        cluster=next(c for c in range(2,self.cluster_count+2) if self.fat(c)==0)
        off=self.offset(cluster)
        self.data[off:off+self.cluster_bytes]=content.ljust(self.cluster_bytes,b'\0')
        for i in range(self.nfat):
            struct.pack_into('<I',self.data,self.fatstart+i*self.fatsz*self.bps+cluster*4,0x0fffffff)
        e=bytearray(32); e[:11]=b'GRUB    CFG'; e[11]=0x20
        struct.pack_into('<H',e,20,cluster>>16); struct.pack_into('<H',e,26,cluster&65535)
        struct.pack_into('<I',e,28,len(content)); self.data[slot:slot+32]=e
        # Unknown hints force a real scan when Linux mounts the ESP later.
        fsinfo=u16(self.data,48); backup=u16(self.data,50)
        for sector in (fsinfo,backup+fsinfo):
            if 0<sector<self.res:
                p=sector*self.bps
                if self.data[p:p+4]==b'RRaA' and self.data[p+484:p+488]==b'rrAa':
                    struct.pack_into('<II',self.data,p+488,0xffffffff,0xffffffff)

def embed_config(loader):
    b=bytearray(loader); pe=u32(b,60)
    assert b[:2]==b'MZ' and b[pe:pe+4]==b'PE\0\0'
    assert u16(b,pe+4)==0x8664 and u16(b,pe+24)==0x20b
    count=u16(b,pe+6); opt=u16(b,pe+20); section=None
    for i in range(count):
        p=pe+24+opt+i*40
        if b[p:p+8].rstrip(b'\0')==b'mods': section=p
    assert section is not None
    raw_size=u32(b,section+16); base=u32(b,section+20)
    magic,padding,start,end=struct.unpack_from('<IIQQ',b,base)
    assert magic==0x676d696d and padding==0 and start==24
    p=start; prefixes=[]
    while p<end:
        typ,size=struct.unpack_from('<II',b,base+p)
        assert size>=8 and p+size<=end
        assert typ!=2, 'Embedded config already present'
        if typ==3: prefixes.append(bytes(b[base+p+8:base+p+size]).rstrip(b'\0'))
        p+=(size+7)//8*8
    assert p==end and prefixes==[b'(,gpt3)/boot/grub']
    # Retain the root-based module prefix. Load the new configuration from the
    # booted EFI directory, using GRUB's documented embedded-config stage.
    script=b'insmod fat\ninsmod normal\nnormal $cmdpath/grub.cfg\n\0'
    module=struct.pack('<II',2,8+len(script))+script
    module=module.ljust((len(module)+7)//8*8,b'\0')
    assert end+len(module)<=raw_size
    assert not any(b[base+end:base+end+len(module)])
    b[base+end:base+end+len(module)]=module
    struct.pack_into('<Q',b,base+16,end+len(module))
    # Standard PE checksum, after all byte edits.
    checksum_offset=pe+24+64
    struct.pack_into('<I',b,checksum_offset,0)
    checksum=sum(struct.unpack('<'+'H'*(len(b)//2),b))
    checksum=(checksum&65535)+(checksum>>16)
    checksum=(checksum&65535)+(checksum>>16)
    struct.pack_into('<I',b,checksum_offset,(checksum+len(b))&0xffffffff)
    assert len(b)==len(loader)
    return bytes(b)

def prepare(esp, expected_loader, config):
    fat=Fat(esp)
    original=fat.read('/EFI/BOOT/BOOTX64.EFI')
    assert hashlib.sha256(original).hexdigest()==expected_loader, 'EFI loader differs from reviewed original'
    fat.add_config(config)
    fat.replace_same_size('/EFI/BOOT/BOOTX64.EFI',embed_config(original))
    verified=Fat(bytes(fat.data))
    assert verified.read('/EFI/BOOT/GRUB.CFG')==config
    assert verified.read('/EFI/BOOT/BOOTX64.EFI')==embed_config(original)
    changes=[]
    for offset in range(0,len(esp),4096):
        before=esp[offset:offset+4096]; after=bytes(fat.data[offset:offset+4096])
        if before!=after: changes.append((offset,before,after))
    assert 0<len(changes)<32
    return bytes(fat.data),changes
