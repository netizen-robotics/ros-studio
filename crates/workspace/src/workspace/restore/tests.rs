use super::*;
use crate::workspace::{Document, Preset};
fn owner(generation: u64, subscription: u64) -> HandleIdentity {
    HandleIdentity {
        generation: Generation(generation),
        subscription: SubscriptionId(subscription),
    }
}
fn preview(coordinator: &mut RestoreCoordinator) -> PreviewId {
    coordinator
        .preview(Preset::Visualize.workspace(), 10, SupportedRenderers::all())
        .unwrap()
}
#[test]
fn multiowner_cleanup_requires_exact_identity_and_yields_exact_inactive_snapshot_once() {
    let current = Preset::Inspect.workspace();
    let mut document = Document::new(current.clone()).unwrap();
    let mut coordinator = RestoreCoordinator::new();
    let mut target = Preset::Visualize.workspace();
    target.layout.reorder("image", 0).unwrap();
    target.layout.scene_percent = 75;
    let id = coordinator
        .preview(target.clone(), 10, SupportedRenderers::all())
        .unwrap();
    assert_eq!(document.current(), &current);
    assert!(coordinator.take_ready(10, &[]).unwrap().is_none());
    let owners = [owner(7, 3), owner(7, 1), owner(7, 4), owner(7, 2)];
    let intents = coordinator
        .apply(id, 10, true, ApplyApproval::DiscardUnsaved, &owners)
        .unwrap();
    assert_eq!(intents.len(), 4);
    assert_eq!(intents[0].owner, owner(7, 1));
    assert_eq!(
        coordinator.apply(id, 10, false, ApplyApproval::CleanOrSaved, &owners),
        Err(RestoreError::ApplyInProgress)
    );
    for unknown in [owner(6, 1), owner(7, 5), owner(8, 2)] {
        assert!(!coordinator.cleanup_confirmed(unknown, 10).unwrap());
    }
    for (index, known) in owners.iter().enumerate() {
        assert!(coordinator.cleanup_confirmed(*known, 10).unwrap());
        assert!(!coordinator.cleanup_confirmed(*known, 10).unwrap());
        if index < 3 {
            assert!(coordinator.take_ready(10, &[]).unwrap().is_none());
        }
    }
    assert_eq!(document.current(), &current);
    let restore = coordinator.take_ready(10, &[]).unwrap().unwrap();
    assert_eq!(restore.configuration(), &target);
    document.apply_inactive(restore);
    assert_eq!(document.current(), &target);
    assert!(!document.is_dirty());
    assert!(coordinator.take_ready(10, &[]).unwrap().is_none());
    assert!(matches!(coordinator.state(), RestoreState::Idle));
    // No runtime identities/status exist inside the returned workspace model.
    let encoded = String::from_utf8(target.encode().unwrap()).unwrap();
    assert!(!encoded.contains("generation"));
    assert!(!encoded.contains("subscription"));
    assert!(!encoded.contains("active"));
}
#[test]
fn failed_stop_requires_verified_cleanup_and_retry_is_explicit_once() {
    let mut coordinator = RestoreCoordinator::new();
    let id = preview(&mut coordinator);
    coordinator
        .apply(
            id,
            10,
            false,
            ApplyApproval::CleanOrSaved,
            &[owner(7, 1), owner(7, 2)],
        )
        .unwrap();
    assert!(coordinator
        .stop_failed(owner(7, 1), 10, &"🤖".repeat(2000))
        .unwrap());
    assert!(!coordinator.stop_failed(owner(6, 1), 10, "stale").unwrap());
    assert!(coordinator.cleanup_confirmed(owner(7, 2), 10).unwrap());
    assert!(coordinator.take_ready(10, &[]).unwrap().is_none());
    let RestoreState::Stopping {
        pending, failures, ..
    } = coordinator.state()
    else {
        panic!("not waiting");
    };
    assert_eq!(pending, &[owner(7, 1)]);
    assert_eq!(failures.len(), 1);
    assert!(failures[0].message.len() <= 512);
    assert!(failures[0].message.ends_with("[truncated]"));
    assert_eq!(
        coordinator.retry_failed(id, 10).unwrap(),
        [StopIntent { owner: owner(7, 1) }]
    );
    assert!(coordinator.retry_failed(id, 10).unwrap().is_empty());
    assert!(coordinator.take_ready(10, &[]).unwrap().is_none());
    assert!(coordinator.cleanup_confirmed(owner(7, 1), 10).unwrap());
    assert!(coordinator.take_ready(10, &[]).unwrap().is_some());
}
#[test]
fn cancelled_apply_and_disconnect_cannot_complete_from_late_acks() {
    let mut coordinator = RestoreCoordinator::new();
    let id = preview(&mut coordinator);
    coordinator
        .apply(id, 10, false, ApplyApproval::CleanOrSaved, &[owner(7, 1)])
        .unwrap();
    coordinator.cancel_apply();
    assert!(!coordinator.cleanup_confirmed(owner(7, 1), 10).unwrap());
    assert!(coordinator.take_ready(10, &[]).unwrap().is_none());
    coordinator
        .apply(id, 10, false, ApplyApproval::CleanOrSaved, &[owner(7, 1)])
        .unwrap();
    coordinator.disconnected();
    assert!(!coordinator.cleanup_confirmed(owner(7, 1), 10).unwrap());
    assert!(coordinator.take_ready(10, &[]).unwrap().is_none());
    assert_eq!(
        coordinator.apply(id, 10, false, ApplyApproval::CleanOrSaved, &[]),
        Err(RestoreError::NoPreview)
    );
    assert!(coordinator.configuration().is_none());
}
#[test]
fn dirty_revision_and_replaced_previews_require_new_explicit_review() {
    let mut coordinator = RestoreCoordinator::new();
    let id = preview(&mut coordinator);
    assert_eq!(
        coordinator.apply(id, 10, true, ApplyApproval::CleanOrSaved, &[]),
        Err(RestoreError::UnsavedChanges)
    );
    assert!(coordinator
        .apply(id, 10, true, ApplyApproval::Cancel, &[])
        .unwrap()
        .is_empty());
    assert!(coordinator.take_ready(10, &[]).unwrap().is_none());
    assert_eq!(
        coordinator.apply(id, 11, false, ApplyApproval::CleanOrSaved, &[]),
        Err(RestoreError::RevisionChanged {
            expected: 10,
            actual: 11
        })
    );
    coordinator.cancel_apply();
    assert!(matches!(
        coordinator.state(),
        RestoreState::ReviewRequired { .. }
    ));
    assert!(coordinator
        .apply(id, 10, false, ApplyApproval::CleanOrSaved, &[])
        .is_err());
    let reviewed = coordinator.configuration().unwrap().clone();
    let newer = coordinator
        .preview(reviewed, 11, SupportedRenderers::all())
        .unwrap();
    assert_ne!(id, newer);
    assert_eq!(
        coordinator.apply(id, 11, false, ApplyApproval::CleanOrSaved, &[]),
        Err(RestoreError::StalePreview)
    );
    coordinator
        .apply(newer, 11, false, ApplyApproval::CleanOrSaved, &[])
        .unwrap();
    assert_eq!(
        coordinator.take_ready(12, &[]).unwrap_err(),
        RestoreError::RevisionChanged {
            expected: 11,
            actual: 12
        }
    );
    assert!(coordinator.take_ready(11, &[]).unwrap().is_none());
}
#[test]
fn preview_replacement_abandons_old_apply_and_capability_rejection_never_filters() {
    let mut coordinator = RestoreCoordinator::new();
    let old = preview(&mut coordinator);
    coordinator
        .apply(old, 10, false, ApplyApproval::CleanOrSaved, &[owner(7, 1)])
        .unwrap();
    let new = coordinator
        .preview(Preset::Analyze.workspace(), 10, SupportedRenderers::all())
        .unwrap();
    assert!(!coordinator.cleanup_confirmed(owner(7, 1), 10).unwrap());
    assert!(coordinator.take_ready(10, &[]).unwrap().is_none());
    let before = coordinator.configuration().unwrap().clone();
    let error = coordinator
        .preview(
            Preset::Visualize.workspace(),
            10,
            SupportedRenderers {
                messages: true,
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(matches!(error, RestoreError::UnsupportedRenderer(_)));
    assert_eq!(coordinator.configuration(), Some(&before));
    coordinator
        .apply(new, 10, false, ApplyApproval::CleanOrSaved, &[])
        .unwrap();
    assert_eq!(
        coordinator
            .take_ready(10, &[])
            .unwrap()
            .unwrap()
            .into_configuration(),
        before
    );
}
#[test]
fn owner_bounds_revision_changes_during_cleanup_and_new_owners_are_safe() {
    let mut coordinator = RestoreCoordinator::new();
    let id = preview(&mut coordinator);
    for owners in [
        vec![owner(7, 1); 2],
        vec![owner(0, 1)],
        vec![owner(7, 0)],
        (1..=33).map(|id| owner(7, id)).collect(),
    ] {
        assert_eq!(
            coordinator.apply(id, 10, false, ApplyApproval::CleanOrSaved, &owners),
            Err(RestoreError::InvalidOwners)
        );
    }
    coordinator
        .apply(id, 10, false, ApplyApproval::CleanOrSaved, &[owner(7, 1)])
        .unwrap();
    assert_eq!(
        coordinator.cleanup_confirmed(owner(7, 1), 11),
        Err(RestoreError::RevisionChanged {
            expected: 10,
            actual: 11
        })
    );
    assert!(coordinator.take_ready(11, &[]).unwrap().is_none());
    let current = coordinator.configuration().unwrap().clone();
    let id = coordinator
        .preview(current, 11, SupportedRenderers::all())
        .unwrap();
    coordinator
        .apply(id, 11, false, ApplyApproval::CleanOrSaved, &[])
        .unwrap();
    assert_eq!(
        coordinator.take_ready(11, &[owner(8, 1)]).unwrap_err(),
        RestoreError::OwnersChanged
    );
    assert!(coordinator.take_ready(11, &[]).unwrap().is_none());
}

#[test]
fn partial_stop_failure_cancel_then_new_review_never_reuses_old_cleanup_or_dirty_approval() {
    let mut coordinator = RestoreCoordinator::new();
    let old = preview(&mut coordinator);
    coordinator
        .apply(
            old,
            10,
            true,
            ApplyApproval::DiscardUnsaved,
            &[owner(7, 1), owner(7, 2)],
        )
        .unwrap();
    coordinator.cleanup_confirmed(owner(7, 1), 10).unwrap();
    coordinator
        .stop_failed(owner(7, 2), 10, "Stop not confirmed")
        .unwrap();
    coordinator.cancel_apply();
    assert!(!coordinator.cleanup_confirmed(owner(7, 2), 10).unwrap());
    assert!(coordinator.take_ready(10, &[]).unwrap().is_none());
    let new = coordinator
        .preview(Preset::Analyze.workspace(), 11, SupportedRenderers::all())
        .unwrap();
    assert_eq!(
        coordinator.apply(new, 11, true, ApplyApproval::CleanOrSaved, &[owner(8, 1)]),
        Err(RestoreError::UnsavedChanges)
    );
    coordinator
        .apply(new, 11, true, ApplyApproval::DiscardUnsaved, &[owner(8, 1)])
        .unwrap();
    assert!(!coordinator.cleanup_confirmed(owner(7, 1), 11).unwrap());
    assert!(!coordinator.cleanup_confirmed(owner(7, 2), 11).unwrap());
    assert!(coordinator.take_ready(11, &[]).unwrap().is_none());
    coordinator.cleanup_confirmed(owner(8, 1), 11).unwrap();
    assert!(coordinator.take_ready(11, &[]).unwrap().is_some());
}
