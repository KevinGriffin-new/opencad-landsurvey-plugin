//! Bulk entity creation for imports and drawn surfaces.
//!
//! Adding entities one at a time costs a host round trip per entity, and
//! tagging each with `write_record` costs another, which the host resolves by
//! looking the handle up in the whole document. That makes a large import
//! quadratic: on host v2026.41 a 100 000-point PNEZD file ran past the host's
//! 30 s dispatch deadline, and the host then marks the plugin dead for the rest
//! of the session ("process died; skipping dispatch", issue #38).
//!
//! A [`Batch`] instead carries each entity's XDATA on the entity itself and
//! adds everything through `add_entities`, in chunks that stay well under the
//! IPC message limit. `add_entities` does not register APPIDs, so
//! [`Batch::commit`] then writes one record per application through
//! `write_record`, which does — that keeps the tags through a DWG/DXF
//! round trip.

use codec::xdata::ExtendedDataRecord;
use codec::{EntityType, Handle};
use ocs_plugin_api::host::HostApi;

/// Entities per `add_entities` call. The host refuses IPC messages over 64 MiB
/// and an oversized `AddEntities` fails without an error, returning no
/// handles; a labelled survey point serializes to a few hundred bytes.
pub(crate) const CHUNK: usize = 5_000;

/// Entities queued for one bulk add.
#[derive(Default)]
pub struct Batch {
    entities: Vec<EntityType>,
    /// The first record queued for each application, with its entity's index,
    /// re-written after the add so the host registers the APPID.
    first_record: Vec<(usize, ExtendedDataRecord)>,
}

impl Batch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue `ent` on `layer`, without XDATA.
    pub fn add(&mut self, mut ent: EntityType, layer: &str) {
        ent.common_mut().layer = layer.to_string();
        self.entities.push(ent);
    }

    /// Queue `ent` on `layer`, carrying `rec`.
    pub fn add_tagged(&mut self, mut ent: EntityType, layer: &str, rec: ExtendedDataRecord) {
        ent.common_mut().layer = layer.to_string();
        if !self
            .first_record
            .iter()
            .any(|(_, r)| r.application_name == rec.application_name)
        {
            self.first_record.push((self.entities.len(), rec.clone()));
        }
        ent.common_mut().extended_data.add_record(rec);
        self.entities.push(ent);
    }

    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// Add every queued entity to the document. Returns how many the host
    /// accepted; when a chunk fails that is less than [`len`](Self::len), and
    /// the shortfall is reported on the command line.
    pub fn commit(self, host: &mut dyn HostApi) -> usize {
        let queued = self.entities.len();
        let mut handles: Vec<Handle> = Vec::with_capacity(self.entities.len());
        let mut rest = self.entities;
        while !rest.is_empty() {
            let tail = rest.split_off(rest.len().min(CHUNK));
            let sent = rest.len();
            let got = host.add_entities(rest);
            let complete = got.len() == sent;
            handles.extend(got);
            if !complete {
                break;
            }
            rest = tail;
        }
        for (index, rec) in self.first_record {
            if let Some(&handle) = handles.get(index) {
                host.write_record(handle, rec);
            }
        }
        if handles.len() < queued {
            host.push_error(&format!(
                "Land Survey: the host accepted {} of {queued} entities; the drawing is incomplete.",
                handles.len()
            ));
        }
        handles.len()
    }
}
