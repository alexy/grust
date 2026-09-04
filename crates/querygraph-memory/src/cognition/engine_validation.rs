//! Validation performed before native cognition planning or execution.

use typesec_memory::Label;

use super::engine::CognitionRequest;
use super::error::CognitionError;
use super::operation::CognitionOperation;

pub(super) fn validate_request(request: &CognitionRequest<'_>) -> Result<(), CognitionError> {
    if !super::bounds::is_canonical_text(request.job_id) {
        return Err(CognitionError::InvalidJobId);
    }
    super::budget::check_authorized_input(request.input.memories())?;
    if request.operation == CognitionOperation::Reconcile {
        super::budget::check_reconcile_work(request.input.memories().len())?;
    }
    request.source.digest()?;
    validate_request_binding(request)
}

fn validate_request_binding(request: &CognitionRequest<'_>) -> Result<(), CognitionError> {
    request
        .field_mapping
        .validate(&request.source.effective_projection)?;
    request
        .binding
        .canonical_digest()
        .map_err(|_| CognitionError::BindingMismatch("invalid TypeSec binding"))?;
    for (name, matches) in [
        (
            "governed scan digest",
            request.binding.governed_scan_digest == request.source.governed_scan_digest,
        ),
        (
            "snapshot digest",
            request.binding.snapshot_digest == request.source.snapshot_digest,
        ),
        (
            "plan task digest",
            request.binding.plan_task_digest == request.source.plan_task_digest,
        ),
        (
            "authorization receipt digest",
            request.binding.authorization_receipt_digest
                == request.source.authorization_receipt_digest,
        ),
        ("subject", request.binding.subject == request.source.subject),
        ("purpose", request.binding.purpose == request.source.purpose),
        (
            "governed source scope",
            request.binding.governed_source_scope.as_ref() == request.input.governed_source_scope(),
        ),
        (
            "source manifest digest",
            request.binding.source_manifest_digest == request.input.manifest().digest,
        ),
    ] {
        if !matches {
            return Err(CognitionError::BindingMismatch(name));
        }
    }

    let mut bound_projection = request.binding.effective_projection.clone();
    let mut source_projection = request.source.effective_projection.clone();
    bound_projection.sort();
    source_projection.sort();
    if bound_projection != source_projection {
        return Err(CognitionError::BindingMismatch("effective projection"));
    }

    let mut recalled: Vec<_> = request
        .input
        .memories()
        .iter()
        .map(|memory| &memory.id)
        .collect();
    recalled.sort();
    let mut manifest: Vec<_> = request
        .input
        .manifest()
        .sources
        .iter()
        .map(|source| &source.id)
        .collect();
    manifest.sort();
    if recalled != manifest {
        return Err(CognitionError::BindingMismatch("source manifest ids"));
    }
    let joined = request
        .input
        .memories()
        .iter()
        .fold(Label::Public, |label, memory| label.join(memory.label));
    if joined != request.input.manifest().joined_label {
        return Err(CognitionError::BindingMismatch("source label join"));
    }
    Ok(())
}
