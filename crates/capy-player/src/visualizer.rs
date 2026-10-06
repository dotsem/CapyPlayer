use futures::StreamExt;
use log::{error, info};
use std::sync::atomic::{AtomicBool, Ordering};
use wayle_cava::{CavaService, InputMethod};

static ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn set_active(active: bool) {
    ACTIVE.store(active, Ordering::Relaxed);
}

pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

pub fn start() {
    std::thread::Builder::new()
        .name("cava-service".to_string())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    error!("Failed to create tokio runtime for cava: {e}");
                    return;
                }
            };

            rt.block_on(async move {
                if let Err(e) = run_cava_loop().await {
                    error!("Cava service error: {e}");
                }
            });
        })
        .expect("Failed to spawn cava thread");
}

async fn run_cava_loop() -> Result<(), Box<dyn std::error::Error>> {
    info!("Starting wayle-cava service");
    let cava = CavaService::builder()
        .bars(24)
        .framerate(30)
        .low_cutoff(50)
        .high_cutoff(10000)
        .autosens(true)
        .noise_reduction(0.65)
        .monstercat(1.5)
        .input(InputMethod::PipeWire)
        .build()
        .await?;

    let mut stream = cava.values.watch();
    let mut frame_count: u64 = 0;
    while let Some(values) = stream.next().await {
        if !is_active() {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            continue;
        }

        frame_count += 1;
        if frame_count % 60 == 0 {
            log::debug!("Cava sample: {:?}", &values[..values.len().min(4)]);
        }

        let total_bars = values.len().max(1) as f32;
        const LOW_CUTOFF: f32 = 50.0;
        const HIGH_CUTOFF: f32 = 10000.0;
        let freq_const = (LOW_CUTOFF / HIGH_CUTOFF).log10() / (1.0 / (total_bars + 1.0) - 1.0);

        let mut bars: Vec<f32> = values
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let raw = if v > 1.0 {
                    (v / 100.0).min(1.0)
                } else {
                    v.max(0.0)
                } as f32;

                // why: progressive treble boost so upper harmonics match bass volume
                let bar_progress = i as f32 / total_bars;
                let treble_boost = 1.0 + 1.2 * bar_progress.powf(1.2);

                // why: gaussian boost centered on 280hz to eliminate the severe 150-500hz low-mid dip
                let log_offset = -freq_const + ((i as f32 + 1.5) / (total_bars + 1.0)) * freq_const;
                let center_freq_hz = HIGH_CUTOFF * 10.0f32.powf(log_offset);
                let log_freq_hz = center_freq_hz.log10();
                let mid_center = 280.0f32.log10();
                let mid_sigma = 0.22f32;
                let mid_bell =
                    (-(log_freq_hz - mid_center).powi(2) / (2.0 * mid_sigma * mid_sigma)).exp();
                let eq_boost = treble_boost + 4.5 * mid_bell;

                (raw * eq_boost).powf(0.55).clamp(0.0, 1.0)
            })
            .collect();

        // why: smooth adjacent frequency bars so narrow notches don't produce harsh visual dips
        let falloff = 1.25f32 * 1.5f32;
        let bar_count = bars.len();
        for peak_idx in 0..bar_count {
            let peak = bars[peak_idx];
            for bar_distance in (0..peak_idx).rev() {
                let de = (peak_idx - bar_distance) as i32;
                bars[bar_distance] = bars[bar_distance].max(peak / falloff.powi(de));
            }
            for bar_distance in (peak_idx + 1)..bar_count {
                let de = (bar_distance - peak_idx) as i32;
                bars[bar_distance] = bars[bar_distance].max(peak / falloff.powi(de));
            }
        }

        crate::events::send_visualizer(bars);
    }

    Ok(())
}
