"""Current development session constants; frozen evidence keeps its own copy."""
VERSION = 21
CAMERA_CAPABILITIES = dict(contract_version=2, max_sample_bytes=25231360,
    max_control_bytes=4096, max_calibration_bytes=8192, max_distortion=16,
    max_owners=1, max_pending=1, max_hz=20,
    pending_timeout_ns=2000000000, retry_interval_ns=50000000)


def camera_waiting(command, revision):
    """Synthetic absent-CameraInfo state; no native ROS or calibrated pixels."""
    config = command['configuration']
    return dict(kind='snapshot', sample=dict(contract_version=2,
        owner=dict(generation=command['generation'], subscription=command['subscription'],
            topic=config['topic'], info=config['output']['camera_snapshot'],
            fixed_frame=config['spatial']['fixed_frame']),
        revision=revision, continuity_epoch=1, status='waiting_info', diagnostic=None,
        counters=dict(images=0, calibrations=0, pending_dropped=0,
            invalid_images=0, invalid_calibrations=0, lookup_timeouts=0), frame=None))
