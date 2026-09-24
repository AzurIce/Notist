use super::text_file::FileRecord;
use crate::{JournalEntry, document::Document};
use anyhow::{Result, bail};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use std::path::Path;

const HEADS: TableDefinition<&str, u64> = TableDefinition::new("document_heads");
const JOURNAL: TableDefinition<(&str, u64), &[u8]> = TableDefinition::new("document_journal");
const TEXT_FILES: TableDefinition<&str, &[u8]> = TableDefinition::new("text_files");

/// Append-only until checkpoint compaction can prove coverage of pending causal
/// packets. Every receipt corresponds to a committed redb transaction.
pub struct Store {
    database: Database,
}
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let database = Database::create(path)?;
        let write = database.begin_write()?;
        {
            write.open_table(HEADS)?;
            write.open_table(JOURNAL)?;
            write.open_table(TEXT_FILES)?;
        }
        write.commit()?;
        Ok(Self { database })
    }
    pub fn commit(&self, document: &str, entry: &JournalEntry) -> Result<()> {
        if entry.packet.identity.document_id != document
            || entry.applied.identity != entry.packet.identity
        {
            bail!("journal identity mismatch");
        }
        let bytes = serde_json::to_vec(entry)?;
        let write = self.database.begin_write()?;
        {
            let mut heads = write.open_table(HEADS)?;
            let previous = heads.get(document)?.map(|v| v.value()).unwrap_or(0);
            if entry.sequence != previous + 1 {
                bail!("journal sequence mismatch");
            }
            write
                .open_table(JOURNAL)?
                .insert((document, entry.sequence), bytes.as_slice())?;
            heads.insert(document, entry.sequence)?;
        }
        write.commit()?;
        Ok(())
    }
    pub fn recover(&self, id: &str) -> Result<Option<(Document, u64)>> {
        let read = self.database.begin_read()?;
        let Some(sequence) = read.open_table(HEADS)?.get(id)?.map(|v| v.value()) else {
            return Ok(None);
        };
        let journal = read.open_table(JOURNAL)?;
        let mut document = None;
        for index in 1..=sequence {
            let record = journal
                .get((id, index))?
                .ok_or_else(|| anyhow::anyhow!("missing journal entry"))?;
            let entry: JournalEntry = serde_json::from_slice(record.value())?;
            if entry.sequence != index || entry.packet.identity.document_id != id {
                bail!("invalid journal record");
            }
            if let Some(doc) = &mut document {
                Document::import(doc, &entry.packet, "recovery".into())?;
            } else {
                document = Some(Document::from_snapshot(&entry.packet, None)?);
            }
            if document.as_ref().unwrap().version() != entry.applied {
                bail!("journal applied version mismatch");
            }
        }
        Ok(document.map(|doc| (doc, sequence)))
    }

    pub(super) fn text_file_record(&self, path: &str) -> Result<Option<FileRecord>> {
        let read = self.database.begin_read()?;
        read.open_table(TEXT_FILES)?
            .get(path)?
            .map(|record| serde_json::from_slice(record.value()).map_err(Into::into))
            .transpose()
    }

    pub(super) fn save_text_file_record(&self, path: &str, record: &FileRecord) -> Result<()> {
        let bytes = serde_json::to_vec(record)?;
        let write = self.database.begin_write()?;
        write
            .open_table(TEXT_FILES)?
            .insert(path, bytes.as_slice())?;
        write.commit()?;
        Ok(())
    }
}
