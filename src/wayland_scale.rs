//! Pure scale state for Wayland outputs. No OS dependency, so the output
//! switch and degradation rules run on every host under `cargo test`.

struct OutputScale {
    id: u32,
    applied: Option<f32>,
    pending: Option<f32>,
}

/// Bound output scales and the outputs the surface currently occupies.
#[derive(Default)]
pub(crate) struct OutputScales {
    outputs: Vec<OutputScale>,
    entered: Vec<u32>,
    applied: Option<f32>,
}

impl OutputScales {
    pub(crate) fn has(&self, id: u32) -> bool {
        self.outputs.iter().any(|output| output.id == id)
    }

    pub(crate) fn bind(&mut self, id: u32) {
        if !self.has(id) {
            self.outputs.push(OutputScale {
                id,
                applied: None,
                pending: None,
            });
        }
    }

    pub(crate) fn scale(&mut self, id: u32, factor: f32) {
        if let Some(output) = self.outputs.iter_mut().find(|output| output.id == id) {
            output.pending = Some(factor);
        }
    }

    fn active_id(&self) -> Option<u32> {
        self.entered
            .last()
            .copied()
            .or_else(|| {
                self.outputs
                    .iter()
                    .find(|output| output.applied.is_some())
                    .map(|output| output.id)
            })
            .or_else(|| self.outputs.first().map(|output| output.id))
    }

    /// Apply a `wl_output.done` frame. The factor is emitted only when the
    /// output is the one the surface currently uses.
    pub(crate) fn done(&mut self, id: u32) -> Option<f32> {
        let factor = {
            let output = self.outputs.iter_mut().find(|output| output.id == id)?;
            let factor = output.pending.take()?;
            output.applied = Some(factor);
            factor
        };
        if Some(id) == self.active_id() {
            self.emit(factor)
        } else {
            None
        }
    }

    pub(crate) fn enter(&mut self, id: u32) -> Option<f32> {
        if self.has(id) && !self.entered.contains(&id) {
            self.entered.push(id);
        }
        self.emit_current()
    }

    pub(crate) fn leave(&mut self, id: u32) -> Option<f32> {
        self.entered.retain(|entered| *entered != id);
        if self.entered.is_empty() {
            self.emit(1.0)
        } else {
            self.emit_current()
        }
    }

    pub(crate) fn emit_current(&mut self) -> Option<f32> {
        let factor = self
            .active_id()
            .and_then(|id| self.outputs.iter().find(|output| output.id == id))
            .and_then(|output| output.applied)
            .unwrap_or(1.0);
        self.emit(factor)
    }

    pub(crate) fn current(&self) -> f32 {
        self.applied.unwrap_or(1.0)
    }

    fn emit(&mut self, factor: f32) -> Option<f32> {
        if self.applied == Some(factor) {
            return None;
        }
        self.applied = Some(factor);
        Some(factor)
    }
}

#[cfg(test)]
mod tests {
    use super::OutputScales;

    #[test]
    fn output_scales_follow_the_entered_output() {
        let mut scales = OutputScales::default();
        scales.bind(1);
        scales.bind(2);
        scales.enter(1);
        scales.scale(1, 2.0);
        assert_eq!(scales.done(1), Some(2.0));
        scales.scale(2, 1.5);
        assert_eq!(scales.done(2), None, "a non-active output must stay quiet");
        assert_eq!(scales.enter(2), Some(1.5));
        assert_eq!(scales.done(1), None, "a stale output must stay quiet");
        assert_eq!(scales.leave(2), Some(2.0));
        assert_eq!(scales.leave(1), Some(1.0));
        assert_eq!(scales.enter(2), Some(1.5), "a known scale is restored");
    }

    #[test]
    fn output_scales_degrade_once_without_an_output() {
        let mut scales = OutputScales::default();
        assert_eq!(scales.emit_current(), Some(1.0));
        assert_eq!(scales.emit_current(), None);
        assert_eq!(scales.current(), 1.0);
    }

    #[test]
    fn output_scales_ignore_unbound_output_ids() {
        let mut scales = OutputScales::default();
        scales.bind(1);
        scales.scale(2, 3.0);
        assert_eq!(scales.done(2), None);
        assert_eq!(scales.enter(2), Some(1.0));
        assert!(!scales.has(2));
    }
}
