# PDF TTS Manual Checklist

## High-Trust PDFs

- [ ] Open a clean selectable-text PDF and wait for TTS analysis to complete.
- [ ] Confirm diagnostics report `high_text_trust` or `mixed_text_trust` with a non-empty sentence plan.
- [ ] Start playback and verify play, pause, resume, stop, previous, next, and direct sentence seek.
- [ ] Verify the active sentence highlight stays on the spoken sentence while audio is playing.
- [ ] Verify follow mode moves the viewport only when the active sentence leaves the visible region.
- [ ] Verify continuous scrolling stays responsive while playback, prefetch, and sync work are active.
- [ ] Verify zoom changes do not corrupt the active highlight position.
- [ ] Verify search activation still jumps correctly while TTS state remains coherent.

## Degraded PDFs

- [ ] Open a mixed-trust PDF and confirm diagnostics expose fallback confidence and reason.
- [ ] Open an OCR-required or scan-first PDF and confirm the configured OCR policy is surfaced.
- [ ] If OCR policy is `disabled`, verify playback is blocked with a clear reason.
- [ ] If OCR policy is `deferred`, verify playback can continue but highlight precision degrades to page follow only.
- [ ] Verify no low-confidence PDF shows exact sentence rectangles when policy forbids them.
- [ ] Verify `render_only_no_sync` documents never pretend sentence-accurate geometry exists.

## Performance

- [ ] Leave playback running in continuous mode and scroll aggressively for at least 30 seconds.
- [ ] Change zoom repeatedly with playback active and confirm viewport responsiveness remains stable.
- [ ] Verify the performance panel stays within the configured activation latency budget during normal use.
- [ ] Confirm cache hits increase over repeated playback of the same sentence window.
- [ ] Confirm stale worker results do not corrupt active playback state after rapid seeking.
