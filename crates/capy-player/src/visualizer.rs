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
            continue;
        }

        frame_count += 1;
        if frame_count % 60 == 0 {
            log::debug!("Cava sample: {:?}", &values[..values.len().min(4)]);
        }

        let total_bars = values.len().max(1) as f32;
        let bars: Vec<f32> = values
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let raw = if v > 1.0 {
                    (v / 100.0).min(1.0)
                } else {
                    v.max(0.0)
                } as f32;
                // why: progressive high-frequency boost so treble matches bass intensity
                let eq_boost = 1.0 + 2.2 * (i as f32 / total_bars).powf(1.2);
                (raw * eq_boost).powf(0.60).clamp(0.0, 1.0)
            })
            .collect();

        crate::events::send_visualizer(bars);
    }

    Ok(())
}
