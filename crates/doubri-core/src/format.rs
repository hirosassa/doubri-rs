use std::io::{Read, Write};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

use crate::error::DoubriError;

/// Magic header for hash files.
pub const HASH_MAGIC: &[u8; 8] = b"DoubriR1";

/// Magic header for index files.
pub const IDX_MAGIC: &[u8; 8] = b"DoubIdR1";

/// Duplicate flag: unique (not duplicate).
pub const FLAG_UNIQUE: u8 = b' ';

/// Duplicate flag: duplicate.
pub const FLAG_DUPLICATE: u8 = b'D';

/// Hash file header information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashFileHeader {
    pub num_documents: u64,
    pub ngram_size: u32,
    pub num_buckets: u32,
    pub band_size: u32,
}

/// Writes a hash file header.
pub fn write_hash_header<W: Write>(
    writer: &mut W,
    header: &HashFileHeader,
) -> Result<(), DoubriError> {
    writer.write_all(HASH_MAGIC)?;
    writer.write_u64::<LittleEndian>(header.num_documents)?;
    writer.write_u32::<LittleEndian>(header.ngram_size)?;
    writer.write_u32::<LittleEndian>(header.num_buckets)?;
    writer.write_u32::<LittleEndian>(header.band_size)?;
    Ok(())
}

/// Reads a hash file header.
pub fn read_hash_header<R: Read>(reader: &mut R) -> Result<HashFileHeader, DoubriError> {
    let mut magic = [0u8; 8];
    reader.read_exact(&mut magic)?;
    if &magic != HASH_MAGIC {
        return Err(DoubriError::InvalidFormat {
            msg: format!("invalid hash file magic: {:?}", magic),
        });
    }
    let num_documents = reader.read_u64::<LittleEndian>()?;
    let ngram_size = reader.read_u32::<LittleEndian>()?;
    let num_buckets = reader.read_u32::<LittleEndian>()?;
    let band_size = reader.read_u32::<LittleEndian>()?;
    Ok(HashFileHeader {
        num_documents,
        ngram_size,
        num_buckets,
        band_size,
    })
}

/// Writes bucket data to a hash file (bucket-major layout).
///
/// `signatures`: flat array of MinHash signatures, length `num_docs * total_hashes`.
/// Document `i`, hash function `j` is at index `i * total_hashes + j`.
/// Output is bucket-major: all documents' values for hash 0, then hash 1, etc.
pub fn write_hash_data<W: Write>(
    writer: &mut W,
    signatures: &[u64],
    num_docs: usize,
    total_hashes: usize,
) -> Result<(), DoubriError> {
    for hash_idx in 0..total_hashes {
        for doc_idx in 0..num_docs {
            writer.write_u64::<LittleEndian>(signatures[doc_idx * total_hashes + hash_idx])?;
        }
    }
    Ok(())
}

/// Reads bucket data from a hash file (bucket-major layout).
///
/// Returns a flat array of MinHash signatures, length `num_docs * total_hashes`.
/// Document `i`, hash function `j` is at index `i * total_hashes + j`.
pub fn read_hash_data<R: Read>(
    reader: &mut R,
    header: &HashFileHeader,
) -> Result<Vec<u64>, DoubriError> {
    let num_docs = header.num_documents as usize;
    let total_hashes = (header.num_buckets * header.band_size) as usize;
    let mut signatures = vec![0u64; num_docs * total_hashes];

    for hash_idx in 0..total_hashes {
        for doc_idx in 0..num_docs {
            signatures[doc_idx * total_hashes + hash_idx] = reader.read_u64::<LittleEndian>()?;
        }
    }

    Ok(signatures)
}

/// Writes a .dup flag file.
pub fn write_dup_flags<W: Write>(writer: &mut W, flags: &[u8]) -> Result<(), DoubriError> {
    writer.write_all(flags)?;
    Ok(())
}

/// Reads a .dup flag file.
pub fn read_dup_flags<R: Read>(reader: &mut R) -> Result<Vec<u8>, DoubriError> {
    let mut flags = Vec::new();
    reader.read_to_end(&mut flags)?;
    Ok(flags)
}

/// Entry in a .src file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceEntry {
    pub item_count: u64,
    pub file_path: String,
}

/// Writes a .src file.
pub fn write_src_file<W: Write>(
    writer: &mut W,
    entries: &[SourceEntry],
) -> Result<(), DoubriError> {
    for entry in entries {
        writeln!(writer, "{}\t{}", entry.item_count, entry.file_path)?;
    }
    Ok(())
}

/// Reads a .src file.
pub fn read_src_file<R: Read>(reader: &mut R) -> Result<Vec<SourceEntry>, DoubriError> {
    let mut content = String::new();
    reader.read_to_string(&mut content)?;
    let mut entries = Vec::new();
    for line in content.lines() {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, '\t');
        let count_str = parts.next().ok_or_else(|| DoubriError::InvalidFormat {
            msg: "missing item count in .src file".to_string(),
        })?;
        let path = parts.next().ok_or_else(|| DoubriError::InvalidFormat {
            msg: "missing file path in .src file".to_string(),
        })?;
        let item_count = count_str
            .parse::<u64>()
            .map_err(|e| DoubriError::InvalidFormat {
                msg: format!("invalid item count '{}': {}", count_str, e),
            })?;
        entries.push(SourceEntry {
            item_count,
            file_path: path.to_string(),
        });
    }
    Ok(entries)
}

/// Index entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    pub group: u32,
    pub item_number: u32,
    pub bucket_value: u64,
}

/// Writes an index file header.
pub fn write_idx_header<W: Write>(writer: &mut W) -> Result<(), DoubriError> {
    writer.write_all(IDX_MAGIC)?;
    Ok(())
}

/// Reads and validates an index file header.
pub fn read_idx_header<R: Read>(reader: &mut R) -> Result<(), DoubriError> {
    let mut magic = [0u8; 8];
    reader.read_exact(&mut magic)?;
    if &magic != IDX_MAGIC {
        return Err(DoubriError::InvalidFormat {
            msg: format!("invalid index file magic: {:?}", magic),
        });
    }
    Ok(())
}

/// Writes an index entry.
pub fn write_idx_entry<W: Write>(writer: &mut W, entry: &IndexEntry) -> Result<(), DoubriError> {
    writer.write_u32::<LittleEndian>(entry.group)?;
    writer.write_u32::<LittleEndian>(entry.item_number)?;
    writer.write_u64::<LittleEndian>(entry.bucket_value)?;
    Ok(())
}

/// Reads an index entry. Returns `None` on EOF.
pub fn read_idx_entry<R: Read>(reader: &mut R) -> Result<Option<IndexEntry>, DoubriError> {
    let group = match reader.read_u32::<LittleEndian>() {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let item_number = reader.read_u32::<LittleEndian>()?;
    let bucket_value = reader.read_u64::<LittleEndian>()?;
    Ok(Some(IndexEntry {
        group,
        item_number,
        bucket_value,
    }))
}

/// Number of index file splits.
pub const NUM_INDEX_SPLITS: usize = 256;

/// Generates an index file path (256-way split).
pub fn idx_file_path(basename: &str, split_idx: u8) -> String {
    format!("{}.idx.{:05}", basename, split_idx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_hash_header_roundtrip() {
        let header = HashFileHeader {
            num_documents: 1000,
            ngram_size: 5,
            num_buckets: 40,
            band_size: 20,
        };
        let mut buf = Vec::new();
        write_hash_header(&mut buf, &header).unwrap();
        let mut cursor = Cursor::new(&buf);
        let read_header = read_hash_header(&mut cursor).unwrap();
        assert_eq!(header, read_header);
    }

    #[test]
    fn test_hash_header_invalid_magic() {
        let buf = b"INVALID!extra_bytes_here____";
        let mut cursor = Cursor::new(&buf[..]);
        let result = read_hash_header(&mut cursor);
        assert!(result.is_err());
    }

    #[test]
    fn test_hash_data_roundtrip() {
        let header = HashFileHeader {
            num_documents: 3,
            ngram_size: 3,
            num_buckets: 2,
            band_size: 2,
        };
        // Flat: doc0=[10,20,30,40], doc1=[11,21,31,41], doc2=[12,22,32,42]
        let signatures: Vec<u64> = vec![
            10, 20, 30, 40, // doc 0
            11, 21, 31, 41, // doc 1
            12, 22, 32, 42, // doc 2
        ];

        let mut buf = Vec::new();
        write_hash_data(&mut buf, &signatures, 3, 4).unwrap();
        let mut cursor = Cursor::new(&buf);
        let read_sigs = read_hash_data(&mut cursor, &header).unwrap();
        assert_eq!(signatures, read_sigs);
    }

    #[test]
    fn test_dup_flags_roundtrip() {
        let flags = vec![FLAG_UNIQUE, FLAG_DUPLICATE, FLAG_UNIQUE, FLAG_DUPLICATE];
        let mut buf = Vec::new();
        write_dup_flags(&mut buf, &flags).unwrap();
        let mut cursor = Cursor::new(&buf);
        let read_flags = read_dup_flags(&mut cursor).unwrap();
        assert_eq!(flags, read_flags);
    }

    #[test]
    fn test_src_file_roundtrip() {
        let entries = vec![
            SourceEntry {
                item_count: 100,
                file_path: "data/file1.hash".to_string(),
            },
            SourceEntry {
                item_count: 200,
                file_path: "data/file2.hash".to_string(),
            },
        ];

        let mut buf = Vec::new();
        write_src_file(&mut buf, &entries).unwrap();
        let mut cursor = Cursor::new(&buf);
        let read_entries = read_src_file(&mut cursor).unwrap();
        assert_eq!(entries, read_entries);
    }

    #[test]
    fn test_idx_entry_roundtrip() {
        let entry = IndexEntry {
            group: 5,
            item_number: 42,
            bucket_value: 0xDEADBEEF,
        };

        let mut buf = Vec::new();
        write_idx_header(&mut buf).unwrap();
        write_idx_entry(&mut buf, &entry).unwrap();

        let mut cursor = Cursor::new(&buf);
        read_idx_header(&mut cursor).unwrap();
        let read_entry = read_idx_entry(&mut cursor).unwrap().unwrap();
        assert_eq!(entry, read_entry);
    }

    #[test]
    fn test_idx_entry_eof() {
        let mut buf = Vec::new();
        write_idx_header(&mut buf).unwrap();

        let mut cursor = Cursor::new(&buf);
        read_idx_header(&mut cursor).unwrap();
        let result = read_idx_entry(&mut cursor).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_idx_file_path() {
        assert_eq!(idx_file_path("output", 0), "output.idx.00000");
        assert_eq!(idx_file_path("output", 255), "output.idx.00255");
        assert_eq!(idx_file_path("data/result", 42), "data/result.idx.00042");
    }

    #[test]
    fn test_multiple_idx_entries_roundtrip() {
        let entries = vec![
            IndexEntry {
                group: 0,
                item_number: 0,
                bucket_value: 100,
            },
            IndexEntry {
                group: 0,
                item_number: 1,
                bucket_value: 200,
            },
            IndexEntry {
                group: 1,
                item_number: 0,
                bucket_value: 100,
            },
        ];

        let mut buf = Vec::new();
        write_idx_header(&mut buf).unwrap();
        for entry in &entries {
            write_idx_entry(&mut buf, entry).unwrap();
        }

        let mut cursor = Cursor::new(&buf);
        read_idx_header(&mut cursor).unwrap();
        let mut read_entries = Vec::new();
        while let Some(entry) = read_idx_entry(&mut cursor).unwrap() {
            read_entries.push(entry);
        }
        assert_eq!(entries, read_entries);
    }

    #[test]
    fn test_hash_file_with_tempfile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.hash");

        let header = HashFileHeader {
            num_documents: 2,
            ngram_size: 5,
            num_buckets: 2,
            band_size: 3,
        };
        // Flat: doc0=[1,2,3,4,5,6], doc1=[7,8,9,10,11,12]
        let signatures: Vec<u64> = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];

        // Write
        {
            let mut file = std::fs::File::create(&path).unwrap();
            write_hash_header(&mut file, &header).unwrap();
            write_hash_data(&mut file, &signatures, 2, 6).unwrap();
        }

        // Read
        {
            let mut file = std::fs::File::open(&path).unwrap();
            let read_header = read_hash_header(&mut file).unwrap();
            assert_eq!(header, read_header);
            let read_sigs = read_hash_data(&mut file, &read_header).unwrap();
            assert_eq!(signatures, read_sigs);
        }
    }
}
