use super::*;

pub struct FinalCaptureMetrics {
    pub duration_ms: u64,
    pub sample_count: usize,
    pub frame_count: usize,
    pub audio_bytes: usize,
}

impl CallbackDiagnostics {
    pub fn observe(&self, samples: &[f32]) {
        if !self.enabled.load(Ordering::Acquire)
            || self.token.as_ref().is_some_and(|token| !token.is_active())
        {
            return;
        }
        let mut block = summarize_samples(samples);
        block.at_ms = self.origin.elapsed().as_millis() as u64;
        if let Err(TrySendError::Full(_)) = self.sender.try_send(Message::Block(block)) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn stream_error(&self, error_name: &'static str) {
        if self.enabled.load(Ordering::Acquire)
            && self.token.as_ref().is_none_or(DiagnosticToken::is_active)
        {
            if let Err(TrySendError::Full(_)) =
                self.sender.try_send(Message::StreamError(error_name))
            {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}
