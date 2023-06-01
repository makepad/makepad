// zip fileformat reading

use std::io::{Read, Seek, SeekFrom};
pub use crate::inflate::decompress_to_vec;

pub const COMPRESS_METHOD_UNCOMPRESSED:u16 = 0;
pub const COMPRESS_METHOD_DEFLATED:u16 = 8;

pub const LOCAL_FILE_HEADER_SIGNATURE:u32 = 0x04034b50;
pub const LOCAL_FILE_HEADER_SIZE:usize = 30;
#[derive(Clone, Debug)]
pub struct LocalFileHeader {
    pub signature: u32,
    pub version_needed_to_extract: u16,
    pub general_purpose_bit_flag: u16,
    pub compression_method: u16,
    pub last_mod_file_time: u16,
    pub last_mod_file_date: u16,
    pub crc32: u32,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    pub file_name_length: u16,
    pub extra_field_length: u16,
    pub file_name: String,
}

impl LocalFileHeader{
    pub fn from_stream(zip_data:&mut (impl Seek+Read))->Result<Self, ZipError>{
        let signature =  read_u32(zip_data)?;
        if signature != LOCAL_FILE_HEADER_SIGNATURE{
            return Err(ZipError::LocalFileHeaderInvalid)
        }
        let version_needed_to_extract = read_u16(zip_data)?;
        let general_purpose_bit_flag = read_u16(zip_data)?;
        let compression_method = read_u16(zip_data)?;
        let last_mod_file_time = read_u16(zip_data)?;
        let last_mod_file_date = read_u16(zip_data)?;
        let crc32 = read_u32(zip_data)?;
        let compressed_size = read_u32(zip_data)?;
        let uncompressed_size = read_u32(zip_data)?;
        let file_name_length = read_u16(zip_data)?;
        let extra_field_length = read_u16(zip_data)?;

        let file_name = read_string(zip_data, file_name_length as usize)?;
        zip_data.seek(SeekFrom::Current(extra_field_length as i64)).map_err(|_| ZipError::CantSeekSkip)?;
        
        Ok(Self{
            signature,
            version_needed_to_extract,
            general_purpose_bit_flag,
            compression_method,
            last_mod_file_time,
            last_mod_file_date,
            crc32,
            compressed_size,
            uncompressed_size,
            file_name_length,
            extra_field_length,
            file_name,
        })
    }
}

pub const CENTRAL_DIR_FILE_HEADER_SIGNATURE:u32 = 0x02014b50;
pub const CENTRAL_DIR_FILE_HEADER_SIZE:usize = 46;
#[derive(Clone, Debug)]
pub struct CentralDirectoryFileHeader {
    pub signature: u32,
    pub version_made_by: u16,
    pub version_needed_to_extract: u16,
    pub general_purpose_bit_flag: u16,
    pub compression_method: u16,
    pub last_mod_file_time: u16,
    pub last_mod_file_date: u16,
    pub crc32: u32,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    pub file_name_length: u16,
    pub extra_field_length: u16,
    pub file_comment_length: u16,
    pub disk_number_start: u16,
    pub internal_file_attributes: u16,
    pub external_file_attributes: u32,
    pub relative_offset_of_local_header: u32,

    pub file_name: String,
    pub file_comment: String,
}

impl CentralDirectoryFileHeader{
    pub fn from_stream(zip_data:&mut (impl Seek+Read))->Result<Self, ZipError>{
        let signature =  read_u32(zip_data)?;
        if signature != CENTRAL_DIR_FILE_HEADER_SIGNATURE{
            return Err(ZipError::CentralDirectoryFileHeaderInvalid)
        }
        let version_made_by = read_u16(zip_data)?;
        let version_needed_to_extract = read_u16(zip_data)?;
        let general_purpose_bit_flag = read_u16(zip_data)?;
        let compression_method = read_u16(zip_data)?;
        let last_mod_file_time = read_u16(zip_data)?;
        let last_mod_file_date = read_u16(zip_data)?;
        let crc32 = read_u32(zip_data)?;
        let compressed_size = read_u32(zip_data)?;
        let uncompressed_size = read_u32(zip_data)?;
        let file_name_length = read_u16(zip_data)?;
        let extra_field_length = read_u16(zip_data)?;
        let file_comment_length = read_u16(zip_data)?;
        let disk_number_start = read_u16(zip_data)?;
        let internal_file_attributes = read_u16(zip_data)?;
        let external_file_attributes = read_u32(zip_data)?;
        let relative_offset_of_local_header = read_u32(zip_data)?;
        let file_name = read_string(zip_data, file_name_length as usize)?;
        zip_data.seek(SeekFrom::Current(extra_field_length as i64)).map_err(|_| ZipError::CantSeekSkip)?;
        let file_comment = read_string(zip_data, file_comment_length as usize)?;
        
        Ok(Self{
            signature,
            version_made_by,
            version_needed_to_extract,
            general_purpose_bit_flag,
            compression_method,
            last_mod_file_time,
            last_mod_file_date,
            crc32,
            compressed_size,
            uncompressed_size,
            file_name_length,
            extra_field_length,
            file_comment_length,
            disk_number_start,
            internal_file_attributes,
            external_file_attributes,
            relative_offset_of_local_header,
            file_name,
            file_comment
        })
    }
}

pub const END_OF_CENTRAL_DIRECTORY_SIGNATURE: u32 = 0x06054b50;
pub const END_OF_CENTRAL_DIRECTORY_SIZE:usize = 22;
#[derive(Clone, Debug)]
pub struct EndOfCentralDirectory {
    pub signature: u32,
    pub number_of_disk: u16,
    pub number_of_start_central_directory_disk: u16,
    pub total_entries_this_disk: u16,
    pub total_entries_all_disk: u16,
    pub size_of_the_central_directory: u32,
    pub central_directory_offset: u32,
    pub zip_file_comment_length: u16,
}

impl EndOfCentralDirectory{
    pub fn from_stream(zip_data:&mut impl Read)->Result<Self, ZipError>{
        let signature =  read_u32(zip_data)?;
        if signature != END_OF_CENTRAL_DIRECTORY_SIGNATURE{
            return Err(ZipError::EndOfCentralDirectoryInvalid)
        }
        Ok(Self{
            signature,
            number_of_disk: read_u16(zip_data)?,
            number_of_start_central_directory_disk: read_u16(zip_data)?,
            total_entries_this_disk: read_u16(zip_data)?,
            total_entries_all_disk: read_u16(zip_data)?,
            size_of_the_central_directory: read_u32(zip_data)?,
            central_directory_offset: read_u32(zip_data)?,
            zip_file_comment_length: read_u16(zip_data)?,
        })
    }
}

fn read_u16(zip_data:&mut impl Read)->Result<u16, ZipError>{
    let mut bytes = [0u8;2];
    if let Ok(size) = zip_data.read(&mut bytes){
        if size != 2{
            return Err(ZipError::DataReadError)
        }
        return Ok(u16::from_le_bytes(bytes))
    }
    Err(ZipError::DataReadError)
}

fn read_u32(zip_data:&mut impl Read)->Result<u32, ZipError>{
    let mut bytes = [0u8;4];
    if let Ok(size) = zip_data.read(&mut bytes){
        if size != 4{
            return Err(ZipError::DataReadError)
        }
        return Ok(u32::from_le_bytes(bytes))
    }
    Err(ZipError::DataReadError)
}

fn read_string(zip_data:&mut impl Read, len:usize)->Result<String, ZipError>{
    let mut data = Vec::new();
    data.resize(len,0u8);
    if let Ok(size) = zip_data.read(&mut data){
        if size != data.len(){
            return Err(ZipError::DataReadError)
        }
        if let Ok(s) = String::from_utf8(data){
            return Ok(s)
        }
        return Err(ZipError::ReadStringError)
    }
    Err(ZipError::DataReadError)
}

fn read_binary(zip_data:&mut impl Read, len:usize)->Result<Vec<u8>, ZipError>{
    let mut data = Vec::new();
    data.resize(len,0u8);
    if let Ok(size) = zip_data.read(&mut data){
        if size != data.len(){
            return Err(ZipError::DataReadError)
        }
        return Ok(data)
    }
    Err(ZipError::DataReadError)
}

pub struct ZipCentralDirectory{
    pub eocd: EndOfCentralDirectory,
    pub file_headers: Vec<CentralDirectoryFileHeader>,
}

impl CentralDirectoryFileHeader{
    // lets read and unzip specific files.
    pub fn extract(&self, zip_data: &mut (impl Seek+Read))->Result<Vec<u8>, ZipError>{
        zip_data.seek(SeekFrom::Start(self.relative_offset_of_local_header as u64)).map_err(|_| ZipError::CantSeekToFileHeader)?;
        let header = LocalFileHeader::from_stream(zip_data)?;
        if header.compression_method == COMPRESS_METHOD_UNCOMPRESSED{
            let decompressed = read_binary(zip_data, self.uncompressed_size as usize)?;
            return Ok(decompressed)
        }
        else if header.compression_method == COMPRESS_METHOD_DEFLATED{
            let compressed = read_binary(zip_data, self.compressed_size as usize)?;
            if let Ok(decompressed) = decompress_to_vec(&compressed){
                return Ok(decompressed);
            }
            else{
                return Err(ZipError::DecompressionError)
            }
        }
        Err(ZipError::UnsupportedCompressionMethod)
    }
}

#[derive(Debug)]
pub enum ZipError{
    LocalFileHeaderInvalid,
    CentralDirectoryFileHeaderInvalid,
    EndOfCentralDirectoryInvalid,
    CantSeekToDirEnd,
    CantSeekToFileHeader,
    CantSeekSkip,
    ParseError,
    ReadStringError,
    CantSeekToDirStart,
    UnsupportedCompressionMethod,
    DecompressionError,
    DataReadError
}

pub fn zip_read_central_directory(zip_data:&mut (impl Seek+Read))->Result<ZipCentralDirectory, ZipError>{
    // lets read the the dirend
    zip_data.seek(SeekFrom::End(-(END_OF_CENTRAL_DIRECTORY_SIZE as i64))).map_err(|_| ZipError::CantSeekToDirEnd)?;
    let eocd = EndOfCentralDirectory::from_stream(zip_data)?;
    zip_data.seek(SeekFrom::Start(eocd.central_directory_offset as u64)).map_err(|_| ZipError::CantSeekToDirStart)?;
    let mut file_headers = Vec::new();
    for _ in 0..eocd.total_entries_all_disk as usize{
        file_headers.push(CentralDirectoryFileHeader::from_stream(zip_data)?);
    }
    Ok(ZipCentralDirectory{
        eocd,
        file_headers
    })
}
