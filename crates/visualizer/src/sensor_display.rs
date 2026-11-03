//! Local presentation lifetime. ROS subscription ownership stays in the root.
use super::*;
use spatial::sensor_history::SensorPoint;
impl ViewState {
    pub fn begin_sensor(&mut self) -> Result<(), String> {
        self.sensor_appearance
            .validate()
            .map_err(|e| e.to_string())?;
        let fixed = &self
            .configuration
            .spatial
            .as_ref()
            .ok_or("Sensor requires scene settings")?
            .fixed_frame;
        self.sensor_token = Some(
            self.sensor_history
                .start(fixed)
                .map_err(|e| e.to_string())?,
        );
        self.sensor_history
            .set_window_ms(self.sensor_appearance.history_ms, self.origin.elapsed())
            .map_err(|e| e.to_string())?;
        self.sensor_appearance_error = None;
        Ok(())
    }
    pub fn receive_sensor(
        &mut self,
        sequence: u64,
        stamp: i64,
        points: Box<[SensorPoint]>,
        ready: bool,
    ) -> Result<(), String> {
        let token = self
            .sensor_token
            .ok_or("Sensor history activation is unavailable")?;
        if !ready {
            self.sensor_history
                .clear(token)
                .map_err(|e| e.to_string())?;
            return Ok(());
        }
        self.sensor_history
            .push(token, sequence, stamp, self.origin.elapsed(), points)
            .map_err(|e| e.to_string())
    }
    pub fn set_sensor_appearance(
        &mut self,
        settings: studio_workspace::sensor::SensorAppearance,
    ) -> Result<(), String> {
        if !matches!(self.kind, ViewKind::Spatial | ViewKind::PointCloud) {
            return Err("Appearance requires a scan/cloud display".into());
        }
        settings.validate().map_err(|e| e.to_string())?;
        self.sensor_history
            .set_window_ms(settings.history_ms, self.origin.elapsed())
            .map_err(|e| e.to_string())?;
        self.sensor_appearance = settings;
        self.sensor_appearance_explicit = true;
        Ok(())
    }
    pub fn clear_sensor(&mut self) -> Result<(), String> {
        if let Some(token) = self.sensor_token {
            self.sensor_history
                .clear(token)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
impl App {
    pub(super) fn poll_sensor_history(&mut self, ctx: &egui::Context) {
        let mut errors = Vec::new();
        for view in &mut self.views {
            if view.sensor_token.is_none() {
                continue;
            }
            let now = view.origin.elapsed();
            if let Err(error) = view.sensor_history.expire(now) {
                errors.push((view.id, error.to_string()));
            }
            if let Some(at) = view.sensor_history.next_expiration() {
                ctx.request_repaint_after(at.saturating_sub(now));
            }
        }
        for (id, error) in errors {
            self.stop_view(id);
            if let Some(view) = self.views.iter_mut().find(|v| v.id == id) {
                view.error = Some(error);
            }
        }
    }
}
