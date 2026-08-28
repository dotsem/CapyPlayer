use std::f32::consts::TAU;
use std::time::Instant;

#[derive(Debug, Clone, Copy, Default)]
pub struct MotionFrame {
    pub angle_deg: f32,
    pub phase: f32,
    pub activity: f32,
}

pub struct PlaybackMotion {
    angle_deg: f32,
    phase: f32,
    velocity_deg_s: f32,
    target_velocity_deg_s: f32,
    last_update: Instant,
}

impl Default for PlaybackMotion {
    fn default() -> Self {
        Self {
            angle_deg: 0.0,
            phase: 0.0,
            velocity_deg_s: 0.0,
            target_velocity_deg_s: 120.0,
            last_update: Instant::now(),
        }
    }
}

impl PlaybackMotion {
    pub fn update(&mut self, is_playing: bool) -> MotionFrame {
        let now = Instant::now();
        let dt = now.duration_since(self.last_update).as_secs_f32().min(0.1);
        self.last_update = now;

        if is_playing {
            let accel_rate = 180.0;
            self.velocity_deg_s = (self.velocity_deg_s + accel_rate * dt).min(self.target_velocity_deg_s);
        } else {
            let decel_rate = 140.0;
            self.velocity_deg_s = (self.velocity_deg_s - decel_rate * dt).max(0.0);
        }

        let activity = self.velocity_deg_s / self.target_velocity_deg_s;

        if self.velocity_deg_s > 0.0 {
            self.angle_deg = (self.angle_deg + self.velocity_deg_s * dt) % 360.0;
            self.phase = (self.phase + (self.velocity_deg_s / 360.0) * TAU * dt) % TAU;
        }

        MotionFrame {
            angle_deg: self.angle_deg,
            phase: self.phase,
            activity,
        }
    }
}
