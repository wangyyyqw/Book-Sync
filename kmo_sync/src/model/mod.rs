use crate::Result;
use prost::Message;

pub mod proto {
    use prost::Message;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, PartialEq, Message, Serialize, Deserialize)]
    pub struct ReadingProgress {
        #[prost(string, tag = "1")]
        pub cfi_position: String,
        #[prost(double, tag = "2")]
        pub progress_percent: f64,
        #[prost(string, tag = "3")]
        pub chapter_id: String,
    }

    #[derive(Clone, PartialEq, Message, Serialize, Deserialize)]
    pub struct Bookmark {
        #[prost(string, tag = "1")]
        pub bookmark_id: String,
        #[prost(string, tag = "2")]
        pub cfi_range: String,
        #[prost(string, tag = "3")]
        pub title: String,
        #[prost(int64, tag = "4")]
        pub create_ts: i64,
    }

    #[derive(Clone, PartialEq, Message, Serialize, Deserialize)]
    pub struct Highlight {
        #[prost(string, tag = "1")]
        pub highlight_id: String,
        #[prost(string, tag = "2")]
        pub cfi_start: String,
        #[prost(string, tag = "3")]
        pub cfi_end: String,
        #[prost(string, tag = "4")]
        pub color: String,
        #[prost(string, tag = "5")]
        pub comment: String,
        #[prost(int64, tag = "6")]
        pub create_ts: i64,
    }

    #[derive(Clone, PartialEq, Message, Serialize, Deserialize)]
    pub struct BookNote {
        #[prost(string, tag = "1")]
        pub note_id: String,
        #[prost(string, tag = "2")]
        pub relate_cfi: String,
        #[prost(string, tag = "3")]
        pub content: String,
        #[prost(int64, tag = "4")]
        pub create_ts: i64,
    }

    #[derive(Clone, PartialEq, Message, Serialize, Deserialize)]
    pub struct BookReadingMeta {
        #[prost(string, tag = "1")]
        pub meta_id: String,
        #[prost(string, tag = "2")]
        pub book_hash: String,
        #[prost(int64, tag = "3")]
        pub modified_ts: i64,
        #[prost(string, tag = "4")]
        pub device_id: String,
        #[prost(message, optional, tag = "5")]
        pub progress: Option<ReadingProgress>,
        #[prost(message, repeated, tag = "6")]
        pub bookmarks: Vec<Bookmark>,
        #[prost(message, repeated, tag = "7")]
        pub highlights: Vec<Highlight>,
        #[prost(message, repeated, tag = "8")]
        pub notes: Vec<BookNote>,
        #[prost(int64, tag = "9")]
        pub wall_clock_ts: i64,
        #[prost(int64, tag = "10")]
        pub logical_ts: i64,
        #[prost(string, tag = "11")]
        pub origin_device_id: String,
        #[prost(message, repeated, tag = "12")]
        pub edit_history: Vec<MetaEdit>,
    }

    #[derive(Clone, PartialEq, Message, Serialize, Deserialize)]
    pub struct MetaEdit {
        #[prost(string, tag = "1")]
        pub edit_id: String,
        #[prost(string, tag = "2")]
        pub device_id: String,
        #[prost(int64, tag = "3")]
        pub logical_ts: i64,
        #[prost(oneof = "meta_edit::Op", tags = "4, 5, 6, 7, 8, 9, 10, 11, 12")]
        pub op: Option<meta_edit::Op>,
    }

    pub mod meta_edit {
        use super::{BookNote, Bookmark, Highlight, ReadingProgress};
        use prost::Oneof;
        use serde::{Deserialize, Serialize};

        #[derive(Clone, PartialEq, Oneof, Serialize, Deserialize)]
        pub enum Op {
            #[prost(message, tag = "4")]
            ProgressUpdate(ReadingProgress),
            #[prost(message, tag = "5")]
            BookmarkAdd(Bookmark),
            #[prost(string, tag = "6")]
            BookmarkRemoveUuid(String),
            #[prost(message, tag = "7")]
            HighlightAdd(Highlight),
            #[prost(message, tag = "8")]
            HighlightUpdate(Highlight),
            #[prost(string, tag = "9")]
            HighlightRemoveUuid(String),
            #[prost(message, tag = "10")]
            NoteAdd(BookNote),
            #[prost(message, tag = "11")]
            NoteUpdate(BookNote),
            #[prost(string, tag = "12")]
            NoteRemoveUuid(String),
        }
    }
}

pub use proto::{BookNote, BookReadingMeta, Bookmark, Highlight, MetaEdit, ReadingProgress};

pub fn encode_meta(meta: &BookReadingMeta) -> Result<Vec<u8>> {
    Ok(meta.encode_to_vec())
}

pub fn decode_meta(bytes: &[u8]) -> Result<BookReadingMeta> {
    Ok(BookReadingMeta::decode(bytes)?)
}

pub fn meta_hash(meta: &BookReadingMeta) -> Result<[u8; 32]> {
    let bytes = encode_meta(meta)?;
    Ok(*blake3::hash(&bytes).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_meta() -> BookReadingMeta {
        BookReadingMeta {
            meta_id: "meta-1".to_string(),
            book_hash: "book-1".to_string(),
            modified_ts: 1,
            device_id: "device-a".to_string(),
            progress: Some(ReadingProgress {
                cfi_position: "epubcfi(/6/2)".to_string(),
                progress_percent: 0.5,
                chapter_id: "chapter-1".to_string(),
            }),
            bookmarks: vec![Bookmark {
                bookmark_id: "bookmark-1".to_string(),
                cfi_range: "range".to_string(),
                title: "Title".to_string(),
                create_ts: 1,
            }],
            highlights: vec![Highlight {
                highlight_id: "highlight-1".to_string(),
                cfi_start: "start".to_string(),
                cfi_end: "end".to_string(),
                color: "yellow".to_string(),
                comment: "important".to_string(),
                create_ts: 1,
            }],
            notes: vec![BookNote {
                note_id: "note-1".to_string(),
                relate_cfi: "cfi".to_string(),
                content: "note".to_string(),
                create_ts: 1,
            }],
            wall_clock_ts: 1,
            logical_ts: 1,
            origin_device_id: "device-a".to_string(),
            edit_history: vec![MetaEdit {
                edit_id: "edit-1".to_string(),
                device_id: "device-a".to_string(),
                logical_ts: 1,
                op: None,
            }],
        }
    }

    #[test]
    fn meta_roundtrip() {
        let meta = sample_meta();
        let bytes = encode_meta(&meta).unwrap();
        let decoded = decode_meta(&bytes).unwrap();
        assert_eq!(decoded, meta);
    }

    #[test]
    fn meta_hash_is_stable_and_sensitive() {
        let mut meta = sample_meta();
        let h1 = meta_hash(&meta).unwrap();
        let h2 = meta_hash(&meta).unwrap();
        assert_eq!(h1, h2);

        meta.logical_ts += 1;
        let h3 = meta_hash(&meta).unwrap();
        assert_ne!(h1, h3);
    }

    #[test]
    fn missing_v4_fields_use_defaults() {
        let old = BookReadingMeta {
            meta_id: "meta-old".to_string(),
            book_hash: "book-old".to_string(),
            modified_ts: 7,
            device_id: "device-old".to_string(),
            progress: None,
            bookmarks: vec![],
            highlights: vec![],
            notes: vec![],
            wall_clock_ts: 0,
            logical_ts: 0,
            origin_device_id: String::new(),
            edit_history: vec![],
        };

        let decoded = decode_meta(&encode_meta(&old).unwrap()).unwrap();
        assert_eq!(decoded.logical_ts, 0);
        assert!(decoded.edit_history.is_empty());
    }
}
