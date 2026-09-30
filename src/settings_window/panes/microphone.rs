use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSButton, NSFont, NSFontWeightRegular, NSPopUpButton, NSSlider, NSTextField, NSView,
};
use objc2_foundation::NSString;

use super::{activate, pane_view, PaneContentHeight};
use crate::audio::available_audio_input_devices;
use crate::config::{UiMeterStyle, MIC_GAIN_MAX_DB, MIC_GAIN_MIN_DB};
use crate::settings_window::actions::SettingsAction;
use crate::settings_window::controls::{
    checkbox, fixed_width, for_auto_layout, is_checked, minimum_width, number_field,
    pop_up_button_with_action, set_checked, set_text_value, set_unsigned_value, slider,
    unsigned_value, NumberFieldKind, NUMBER_FIELD_WIDTH,
};
use crate::settings_window::form::{mic_gain_label, mic_gain_load_problem, MicrophoneForm};
use crate::settings_window::grid::{ControlWidth, FormGrid, RowAlignment};
use crate::settings_window::helpers::{mic_audio_device_popup_state, MicAudioDeviceOption};
use crate::state::MicMeterSnapshot;
use crate::ui_meter::{meter_container_height, UiMeterView, METER_BORDER_PADDING};

const GAIN_SLIDER_MIN_WIDTH: f64 = 150.0;
/// Width the preview meter bars are laid out in.
const METER_WIDTH: f64 = 180.0;
const ALWAYS_ON_HINT: &str = "Prevents delay at the cost of 1-1.5% CPU.";

#[derive(Debug)]
pub struct MicrophonePane {
    audio_device_popup: Retained<NSPopUpButton>,
    audio_device_options: RefCell<Vec<MicAudioDeviceOption>>,
    sample_rate_field: Retained<NSTextField>,
    gain_slider: Retained<NSSlider>,
    gain_label: Retained<NSTextField>,
    meter_view: UiMeterView,
    hold_ms_field: Retained<NSTextField>,
    always_on_checkbox: Retained<NSButton>,
}

impl MicrophonePane {
    pub fn new(mtm: MainThreadMarker, target: &AnyObject) -> (Self, Retained<NSView>) {
        let audio_device_popup =
            pop_up_button_with_action(mtm, target, SettingsAction::MicAudioDeviceChanged);
        let sample_rate_field = number_field(
            mtm,
            NumberFieldKind::UnsignedInteger {
                maximum: u64::from(u32::MAX),
            },
        );
        let gain_slider = slider(
            mtm,
            target,
            SettingsAction::MicGainSliderChanged,
            f64::from(MIC_GAIN_MIN_DB),
            f64::from(MIC_GAIN_MAX_DB),
        );
        let gain_label = gain_value_label(mtm, gain_slider.doubleValue() as f32);
        let meter_view = UiMeterView::new(mtm, UiMeterStyle::AnimatedColor);
        // `NSNumberFormatter` rejects integers above `i64::MAX`, so that is the
        // largest silence pad the field can take.
        let hold_ms_field = number_field(
            mtm,
            NumberFieldKind::UnsignedInteger {
                maximum: i64::MAX as u64,
            },
        );
        let always_on_checkbox = checkbox(mtm, "Keep microphone connection open");

        fixed_width(&sample_rate_field, NUMBER_FIELD_WIDTH);
        fixed_width(&hold_ms_field, NUMBER_FIELD_WIDTH);
        minimum_width(&gain_slider, GAIN_SLIDER_MIN_WIDTH);
        let meter = for_auto_layout(meter_view.view());
        activate(&[
            // The container frames the bars with its border padding on both
            // sides, as `UiMeterView` lays them out.
            meter
                .widthAnchor()
                .constraintEqualToConstant(METER_WIDTH + METER_BORDER_PADDING * 2.0),
            meter
                .heightAnchor()
                .constraintEqualToConstant(meter_container_height(UiMeterStyle::AnimatedColor)),
        ]);

        let grid = FormGrid::new(mtm, 2);
        grid.add_row(
            "Audio device:",
            &audio_device_popup,
            ControlWidth::Intrinsic,
            &[],
        );
        grid.add_row(
            "Sample rate:",
            &sample_rate_field,
            ControlWidth::Intrinsic,
            &[],
        );
        grid.add_aligned_row(
            "Mic gain (dB):",
            &gain_slider,
            ControlWidth::Fill,
            &[&gain_label, meter],
            RowAlignment::Center,
        );
        grid.add_row(
            "Silence pad (ms):",
            &hold_ms_field,
            ControlWidth::Intrinsic,
            &[],
        );
        grid.add_detail_row(&always_on_checkbox, ControlWidth::Intrinsic);
        grid.add_hint_row(Some(ALWAYS_ON_HINT));

        let pane = Self {
            audio_device_popup,
            audio_device_options: RefCell::new(Vec::new()),
            sample_rate_field,
            gain_slider,
            gain_label,
            meter_view,
            hold_ms_field,
            always_on_checkbox,
        };
        let view = pane_view(mtm, grid.view(), PaneContentHeight::Fitting);
        (pane, view)
    }

    /// Loads every field of the form, listing the current audio input
    /// devices, and returns the problems to show in the status area: a failed
    /// device listing (#10) and a gain the slider cannot show.
    pub fn load(&self, form: &MicrophoneForm) -> Vec<String> {
        let audio_device_problem = self.populate_audio_device_popup(form.audio_device.as_deref());
        set_unsigned_value(&self.sample_rate_field, form.sample_rate);
        self.gain_slider.setDoubleValue(f64::from(form.gain));
        // The label shows the slider value, which is what saving writes.
        self.update_gain_label(self.gain_slider_value());
        set_unsigned_value(&self.hold_ms_field, form.hold_ms);
        set_checked(&self.always_on_checkbox, form.always_on);
        audio_device_problem
            .into_iter()
            .chain(mic_gain_load_problem(form.gain))
            .collect()
    }

    pub fn read(&self) -> MicrophoneForm {
        MicrophoneForm {
            audio_device: self.audio_device_value(),
            sample_rate: unsigned_value(&self.sample_rate_field),
            gain: self.gain_slider_value(),
            hold_ms: unsigned_value(&self.hold_ms_field),
            always_on: is_checked(&self.always_on_checkbox),
        }
    }

    pub fn gain_slider_value(&self) -> f32 {
        self.gain_slider.doubleValue() as f32
    }

    pub fn update_gain_label(&self, gain_db: f32) {
        set_text_value(&self.gain_label, &mic_gain_label(gain_db));
    }

    pub fn update_meter(&self, meter: MicMeterSnapshot) {
        // Settings shows the fixed-scale meter, which draws from the level alone.
        self.meter_view.update(meter, METER_WIDTH, &[]);
    }

    pub fn audio_device_value(&self) -> Option<String> {
        let selected_title = self
            .audio_device_popup
            .titleOfSelectedItem()
            .map(|selected_title| selected_title.to_string())?;

        if let Some(option) = self
            .audio_device_options
            .borrow()
            .iter()
            .find(|option| option.title == selected_title)
        {
            return option.value.clone();
        }

        let trimmed_value = selected_title.trim();
        if trimmed_value.is_empty() {
            None
        } else {
            Some(trimmed_value.to_owned())
        }
    }

    /// Lists the audio input devices in the popup and selects the configured
    /// one. Returns the device listing error, if any, as a status message.
    fn populate_audio_device_popup(&self, configured_audio_device: Option<&str>) -> Option<String> {
        let state =
            mic_audio_device_popup_state(available_audio_input_devices(), configured_audio_device);

        self.audio_device_popup.removeAllItems();
        for option in &state.options {
            self.audio_device_popup
                .addItemWithTitle(&NSString::from_str(&option.title));
        }
        self.audio_device_popup
            .selectItemWithTitle(&NSString::from_str(&state.selected_title));
        self.audio_device_options.replace(state.options);

        state.load_problem
    }
}

/// Gain value label showing `initial_gain_db`. It is sized to the label for
/// the highest gain, the widest text it shows, so the row does not shift while
/// the slider moves.
fn gain_value_label(mtm: MainThreadMarker, initial_gain_db: f32) -> Retained<NSTextField> {
    let label = for_auto_layout(NSTextField::labelWithString(
        &NSString::from_str(&mic_gain_label(MIC_GAIN_MAX_DB)),
        mtm,
    ));
    label.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
        NSFont::systemFontSize(),
        unsafe { NSFontWeightRegular },
    )));
    minimum_width(&label, label.fittingSize().width);
    set_text_value(&label, &mic_gain_label(initial_gain_db));
    label
}
