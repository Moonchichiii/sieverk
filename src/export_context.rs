//! Verified export context (SK-SIE-BRIDGE-03).
//!
//! The one place where the accounting identity and fiscal period of a snapshot
//! become available to an export writer. A context exists only after
//! `bind` has proven that the snapshot it was built from is exactly the one
//! the workspace and its verified engine run were built from; it is always
//! complete (L-CTX-COMPLETE), lives only in process memory, is never
//! serialised or persisted, and its `Debug` never prints the legal name or the
//! identifier. Redaction is not removal: the writer reads the original values
//! through the getters to produce the authorised artefact (`#FNAMN`, `#ORGNR`,
//! `#RAR 0` in BRIDGE-04; later SRU).
//!
//! There is no fallback anywhere: not `display_name`, not a legacy
//! `org_number`, not CLI values, not a period derived from `income_year`.

use std::fmt;

use crate::snapshot::{EntityType, FiscalPeriod, IdentifierKind};
use crate::workspace::{IngestedSnapshot, Workspace, WorkspaceError};

/// A complete, provenance-bound export context. No `Clone`, no `Serialize`,
/// no `Display`.
pub struct VerifiedExportContext {
    legal_name: String,
    entity_type: EntityType,
    identifier_kind: IdentifierKind,
    business_identifier: String,
    fiscal_period: FiscalPeriod,
    snapshot_sha256: String,
}

impl VerifiedExportContext {
    /// Bind in the locked order (BRIDGE-03 prespec §6), first failure wins:
    /// 1. `verify_run()` must accept the run;
    /// 2. envelope digest == workspace snapshot digest;
    /// 3. envelope digest == the verified run's provenance digest;
    /// 4. accounting identity present;
    /// 5. fiscal period present.
    pub fn bind(ingested: &IngestedSnapshot, ws: &Workspace) -> Result<Self, ExportContextError> {
        let run = ws
            .verify_run()
            .map_err(ExportContextError::RunNotVerified)?;
        let digest = ingested.snapshot_sha256();
        let workspace_digest = ws
            .snapshot_sha256()
            .map_err(ExportContextError::RunNotVerified)?;
        if digest != workspace_digest {
            return Err(ExportContextError::ProvenanceMismatch { point: "workspace" });
        }
        // Defence in depth: verify_run() already requires engine_run_meta to
        // name the workspace snapshot, so with a valid workspace this branch is
        // not reachable; it stays as an explicit statement of the contract.
        if digest != run.provenance.snapshot_sha256 {
            return Err(ExportContextError::ProvenanceMismatch {
                point: "engine_run",
            });
        }
        let snapshot = ingested.snapshot();
        let identity = snapshot
            .entity
            .accounting_identity
            .as_ref()
            .ok_or(ExportContextError::IdentityMissing)?;
        let fiscal_period = snapshot
            .fiscal_period
            .ok_or(ExportContextError::FiscalPeriodMissing)?;
        Ok(Self {
            legal_name: identity.legal_name.clone(),
            entity_type: identity.entity_type,
            identifier_kind: identity.identifier_kind,
            business_identifier: identity.business_identifier.clone(),
            fiscal_period,
            snapshot_sha256: digest.to_string(),
        })
    }

    /// The legal name exactly as sent (for `#FNAMN`).
    pub fn legal_name(&self) -> &str {
        &self.legal_name
    }

    pub fn entity_type(&self) -> EntityType {
        self.entity_type
    }

    pub fn identifier_kind(&self) -> IdentifierKind {
        self.identifier_kind
    }

    /// The identifier exactly as sent, wire form `NNNNNN-NNNN` (for `#ORGNR`).
    pub fn business_identifier(&self) -> &str {
        &self.business_identifier
    }

    /// The fiscal period exactly as sent (for `#RAR 0`).
    pub fn fiscal_period(&self) -> FiscalPeriod {
        self.fiscal_period
    }

    pub fn snapshot_sha256(&self) -> &str {
        &self.snapshot_sha256
    }
}

impl fmt::Debug for VerifiedExportContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VerifiedExportContext")
            .field("entity_type", &self.entity_type)
            .field("identifier_kind", &self.identifier_kind)
            .field("fiscal_period", &self.fiscal_period)
            .field("snapshot_sha256", &self.snapshot_sha256)
            .field("legal_name", &format_args!("[REDACTED]"))
            .field("business_identifier", &format_args!("[REDACTED]"))
            .finish()
    }
}

/// Why a context could not be bound. Texts are fixed: never an identifier or a
/// legal name.
#[derive(Debug)]
pub enum ExportContextError {
    /// No ingested snapshot, no engine run, a corrupt run, or a run the
    /// authoritative verifier refuses (including a run that names another
    /// snapshot).
    RunNotVerified(WorkspaceError),
    /// The envelope's digest does not match the workspace or the run.
    ProvenanceMismatch { point: &'static str },
    /// The snapshot carries no accounting identity (1.0/1.1, or 1.2 with null).
    IdentityMissing,
    /// The snapshot carries no fiscal period (1.0/1.1, or 1.2 with null).
    FiscalPeriodMissing,
}

impl fmt::Display for ExportContextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Fixed text; the underlying workspace error stays available via source().
            Self::RunNotVerified(_) => write!(f, "no verified engine run for this workspace"),
            Self::ProvenanceMismatch { point } => write!(
                f,
                "snapshot provenance mismatch ({point}): the snapshot is not the one this workspace was built from"
            ),
            Self::IdentityMissing => write!(f, "the snapshot carries no accounting identity"),
            Self::FiscalPeriodMissing => write!(f, "the snapshot carries no fiscal period"),
        }
    }
}

impl std::error::Error for ExportContextError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::RunNotVerified(e) => Some(e),
            _ => None,
        }
    }
}
