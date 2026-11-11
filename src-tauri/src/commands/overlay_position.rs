pub fn place_at_bottom_center(
    window: &tauri::WebviewWindow,
    width: f64,
    height: f64,
    bottom_gap: f64,
    anchor_width: Option<f64>,
) -> Result<(), String> {
    let monitor = overlay_monitor(window)?;

    let position = bottom_center_position(
        monitor.work_area(),
        monitor.scale_factor(),
        width,
        height,
        bottom_gap,
        anchor_width,
    );
    window
        .set_position(position)
        .map_err(|error| error.to_string())
}

fn overlay_monitor(window: &tauri::WebviewWindow) -> Result<tauri::Monitor, String> {
    window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten())
        .ok_or_else(|| "No monitor available for overlay placement".to_string())
}

fn bottom_center_position(
    work_area: &tauri::PhysicalRect<i32, u32>,
    scale_factor: f64,
    width: f64,
    height: f64,
    bottom_gap: f64,
    anchor_width: Option<f64>,
) -> tauri::LogicalPosition<f64> {
    let anchor_x = work_area.position.x as f64 / scale_factor;
    let anchor_y = work_area.position.y as f64 / scale_factor;
    let anchor_w = work_area.size.width as f64 / scale_factor;
    let anchor_h = work_area.size.height as f64 / scale_factor;
    let x = anchor_x + ((anchor_w - anchor_width.unwrap_or(width)) / 2.0).round();
    let y = anchor_y + anchor_h - height - bottom_gap;

    clamp_position(
        work_area,
        scale_factor,
        width,
        height,
        tauri::LogicalPosition::new(x, y),
    )
}

fn cell_preserving_position(
    work_area: &tauri::PhysicalRect<i32, u32>,
    scale_factor: f64,
    width: f64,
    height: f64,
    outer_position: tauri::PhysicalPosition<i32>,
    source_cell_offset: tauri::LogicalPosition<f64>,
) -> tauri::PhysicalPosition<i32> {
    let source_offset = source_cell_offset.to_physical::<i32>(scale_factor);
    // Expanded details retain the same virtual bottom-centred 80x80 cell as the orb.
    let target_offset = tauri::LogicalPosition::new((width - 80.0) / 2.0, height - 80.0)
        .to_physical::<i32>(scale_factor);
    let x = outer_position.x + source_offset.x - target_offset.x;
    let y = outer_position.y + source_offset.y - target_offset.y;
    let target_size = tauri::LogicalSize::new(width, height).to_physical::<u32>(scale_factor);
    let origin = work_area.position;
    tauri::PhysicalPosition::new(
        x.clamp(
            origin.x,
            origin.x + work_area.size.width.saturating_sub(target_size.width) as i32,
        ),
        y.clamp(
            origin.y,
            origin.y + work_area.size.height.saturating_sub(target_size.height) as i32,
        ),
    )
}

fn clamp_position(
    work_area: &tauri::PhysicalRect<i32, u32>,
    scale_factor: f64,
    width: f64,
    height: f64,
    position: tauri::LogicalPosition<f64>,
) -> tauri::LogicalPosition<f64> {
    let origin = work_area.position.to_logical::<f64>(scale_factor);
    let size = work_area.size.to_logical::<f64>(scale_factor);
    tauri::LogicalPosition::new(
        position
            .x
            .clamp(origin.x, origin.x + (size.width - width).max(0.0)),
        position
            .y
            .clamp(origin.y, origin.y + (size.height - height).max(0.0)),
    )
}

#[tauri::command]
pub fn position_overlay(
    window: tauri::WebviewWindow,
    width: f64,
    height: f64,
    bottom_gap: f64,
    keep_cell: bool,
    source_cell_offset: tauri::LogicalPosition<f64>,
) -> Result<(), String> {
    super::window_authorization::require_overlay(&window)?;
    if !keep_cell {
        return place_at_bottom_center(&window, width, height, bottom_gap, None);
    }
    let monitor = overlay_monitor(&window)?;
    let position = cell_preserving_position(
        monitor.work_area(),
        monitor.scale_factor(),
        width,
        height,
        window.outer_position().map_err(|error| error.to_string())?,
        source_cell_offset,
    );
    window
        .set_position(position)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{bottom_center_position, cell_preserving_position};

    #[test]
    fn preview_keeps_the_compact_orb_screen_rectangle() {
        for (origin, size, scale) in [
            ((0, 0), (1440, 900), 1.0),
            ((-2560, 100), (2560, 1400), 2.0),
            ((1920, -1440), (2880, 1800), 1.5),
        ] {
            let work_area = tauri::PhysicalRect {
                position: tauri::PhysicalPosition::new(origin.0, origin.1),
                size: tauri::PhysicalSize::new(size.0, size.1),
            };
            let compact = bottom_center_position(&work_area, scale, 80.0, 80.0, 0.0, None);
            let preview = bottom_center_position(&work_area, scale, 380.0, 144.0, 0.0, None);
            // The preview's bottom-centred 80x80 cell starts at (150, 64).
            assert_eq!(compact.x, preview.x + 150.0);
            assert_eq!(compact.y, preview.y + 64.0);
        }
    }

    #[test]
    fn placement_converts_scaled_and_offset_work_areas_before_centering() {
        let work_area = tauri::PhysicalRect {
            position: tauri::PhysicalPosition::new(-2560, 100),
            size: tauri::PhysicalSize::new(2560, 1400),
        };
        assert_eq!(
            bottom_center_position(&work_area, 2.0, 380.0, 144.0, 0.0, None),
            tauri::LogicalPosition::new(-830.0, 606.0)
        );
        assert_eq!(
            bottom_center_position(&work_area, 2.0, 380.0, 210.0, 28.0, None),
            tauri::LogicalPosition::new(-830.0, 512.0)
        );
        assert_eq!(
            bottom_center_position(&work_area, 2.0, 380.0, 144.0, 0.0, Some(80.0)),
            tauri::LogicalPosition::new(-680.0, 606.0)
        );
    }

    #[test]
    fn initial_placement_pins_undersized_axes_to_work_area_origin() {
        for (size, expected) in [
            ((300, 900), (-500.0, 756.0)),
            ((1440, 100), (30.0, 0.0)),
            ((300, 100), (-500.0, 0.0)),
        ] {
            let work_area = tauri::PhysicalRect {
                position: tauri::PhysicalPosition::new(-500, 0),
                size: tauri::PhysicalSize::new(size.0, size.1),
            };
            let position = bottom_center_position(&work_area, 1.0, 380.0, 144.0, 0.0, None);
            assert_eq!(
                position,
                tauri::LogicalPosition::new(expected.0, expected.1)
            );
        }
    }

    #[test]
    fn dragged_cell_survives_every_shape_transition_on_scaled_monitors() {
        let shapes = [(80.0, 80.0), (380.0, 144.0), (380.0, 210.0)];
        for (origin, size, scale, cell) in [
            ((0, 0), (1440, 900), 1.0, (400.0, 300.0)),
            ((-2560, 100), (2560, 1400), 2.0, (-900.0, 350.0)),
            ((1920, -1440), (2880, 1800), 1.5, (1800.0, -600.0)),
        ] {
            let work_area = tauri::PhysicalRect {
                position: tauri::PhysicalPosition::new(origin.0, origin.1),
                size: tauri::PhysicalSize::new(size.0, size.1),
            };
            for (source_width, source_height) in shapes {
                let source_offset =
                    tauri::LogicalPosition::new((source_width - 80.0) / 2.0, source_height - 80.0);
                let outer =
                    tauri::LogicalPosition::new(cell.0 - source_offset.x, cell.1 - source_offset.y)
                        .to_physical::<i32>(scale);
                for (width, height) in shapes {
                    let position = cell_preserving_position(
                        &work_area,
                        scale,
                        width,
                        height,
                        outer,
                        source_offset,
                    );
                    let target_offset =
                        tauri::LogicalPosition::new((width - 80.0) / 2.0, height - 80.0)
                            .to_physical::<i32>(scale);
                    let expected_cell =
                        tauri::LogicalPosition::new(cell.0, cell.1).to_physical::<i32>(scale);
                    assert_eq!(position.x + target_offset.x, expected_cell.x);
                    assert_eq!(position.y + target_offset.y, expected_cell.y);
                }
            }
        }
    }

    #[test]
    fn fractional_scale_shape_cycles_keep_the_exact_physical_cell() {
        let work_area = tauri::PhysicalRect {
            position: tauri::PhysicalPosition::new(-500, 100),
            size: tauri::PhysicalSize::new(3000, 2000),
        };
        let transitions = [
            (380.0, 144.0, (0.0, 0.0), (150.0, 64.0)),
            (380.0, 210.0, (150.0, 64.0), (150.0, 130.0)),
            (380.0, 144.0, (150.0, 130.0), (150.0, 64.0)),
            (80.0, 80.0, (150.0, 64.0), (0.0, 0.0)),
        ];
        for scale in [1.25, 1.75] {
            for y in 1000..1010 {
                let start = tauri::PhysicalPosition::new(900, y);
                let mut outer = start;
                for _ in 0..10 {
                    for (width, height, source, target) in transitions {
                        let position: tauri::Position = cell_preserving_position(
                            &work_area,
                            scale,
                            width,
                            height,
                            outer,
                            tauri::LogicalPosition::new(source.0, source.1),
                        )
                        .into();
                        outer = position.to_physical::<i32>(scale);
                        let offset = tauri::LogicalPosition::new(target.0, target.1)
                            .to_physical::<i32>(scale);
                        assert_eq!(
                            tauri::PhysicalPosition::new(outer.x + offset.x, outer.y + offset.y),
                            start,
                            "cell drift at scale {scale}, start y {y}, target {width}x{height}"
                        );
                    }
                    assert_eq!(outer, start);
                }
            }
        }
    }

    #[test]
    fn preserved_placement_clamps_each_edge_and_pins_undersized_axes() {
        for (scale, size, cell, expected) in [
            (1.0, (1000, 800), (100, 100), (100, 36)),
            (1.0, (1000, 800), (1100, 100), (720, 36)),
            (1.0, (1000, 800), (500, -50), (350, -50)),
            (1.0, (1000, 800), (500, 800), (350, 606)),
            (1.0, (300, 800), (200, 300), (100, 236)),
            (1.0, (1000, 100), (500, 0), (350, -50)),
            (1.0, (300, 100), (200, 0), (100, -50)),
            (1.25, (1000, 800), (100, 100), (100, 20)),
            (1.25, (1000, 800), (1100, 100), (625, 20)),
            (1.25, (1000, 800), (500, -50), (312, -50)),
            (1.25, (1000, 800), (500, 800), (312, 570)),
            (1.25, (300, 100), (200, 0), (100, -50)),
            (1.75, (1000, 800), (100, 100), (100, -12)),
            (1.75, (1000, 800), (1100, 100), (435, -12)),
            (1.75, (1000, 800), (500, -50), (237, -50)),
            (1.75, (1000, 800), (500, 800), (237, 498)),
            (1.75, (300, 100), (200, 0), (100, -50)),
        ] {
            let work_area = tauri::PhysicalRect {
                position: tauri::PhysicalPosition::new(100, -50),
                size: tauri::PhysicalSize::new(size.0, size.1),
            };
            let position: tauri::Position = cell_preserving_position(
                &work_area,
                scale,
                380.0,
                144.0,
                tauri::PhysicalPosition::new(cell.0, cell.1),
                tauri::LogicalPosition::new(0.0, 0.0),
            )
            .into();
            assert_eq!(
                position.to_physical::<i32>(scale),
                tauri::PhysicalPosition::new(expected.0, expected.1)
            );
        }
    }
}
