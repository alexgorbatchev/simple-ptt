//! Uniform overlay motion. Layout stays in points; Core Animation scales
//! the rendered glass, halo, text, and meter together about their centre.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, DefinedClass, MainThreadOnly};
use objc2_app_kit::{NSView, NSViewLayerContentsRedrawPolicy};
use objc2_foundation::{ns_string, MainThreadMarker, NSArray, NSNumber, NSRect, NSValue};
use objc2_quartz_core::{
    kCAMediaTimingFunctionEaseInEaseOut, kCAMediaTimingFunctionEaseOut, CABasicAnimation,
    CAKeyframeAnimation, CAMediaTiming, CAMediaTimingFunction, CATransaction,
    NSValueCATransform3DAdditions,
};

use super::glass::centered_scale;

const MOTION_KEY: &str = "simple-ptt.overlay-scale";

#[derive(Clone, Copy, Debug)]
enum Motion {
    Set,
    Scale { seconds: f64 },
    Pop { seconds: f64, start: f64 },
}

#[derive(Debug)]
pub(super) struct MotionIvars {
    scale: Cell<f64>,
    pending: Cell<Option<Motion>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "SimplePttOverlayMotionView"]
    #[ivars = MotionIvars]
    #[derive(Debug)]
    pub(super) struct OverlayMotionView;

    impl OverlayMotionView {
        #[unsafe(method(wantsUpdateLayer))]
        fn wants_update_layer(&self) -> bool {
            true
        }

        #[unsafe(method(updateLayer))]
        fn update_layer(&self) {
            let Some(layer) = self.layer() else { return; };
            let pending = self.ivars().pending.take();
            let target = centered_scale(layer.bounds(), layer.anchorPoint(), self.ivars().scale.get());
            // SAFETY: the presentation layer is read on the main thread.
            let current = unsafe { layer.presentationLayer() }
                .map_or_else(|| layer.transform(), |presentation| presentation.transform());
            CATransaction::begin();
            CATransaction::setDisableActions(true);
            layer.setTransform(target);
            if let Some(motion) = pending {
                match motion {
                    Motion::Set => layer.removeAnimationForKey(ns_string!(MOTION_KEY)),
                    Motion::Scale { seconds } => {
                        let animation = CABasicAnimation::animationWithKeyPath(Some(ns_string!("transform")));
                        // SAFETY: transform animations take boxed CATransform3D values.
                        unsafe {
                            animation.setFromValue(Some(&NSValue::valueWithCATransform3D(current)));
                            animation.setToValue(Some(&NSValue::valueWithCATransform3D(target)));
                        }
                        animation.setDuration(seconds);
                        animation.setTimingFunction(Some(&CAMediaTimingFunction::functionWithName(
                            unsafe { kCAMediaTimingFunctionEaseInEaseOut },
                        )));
                        layer.addAnimation_forKey(&animation, Some(ns_string!(MOTION_KEY)));
                    }
                    Motion::Pop { seconds, start } => {
                        let animation = CAKeyframeAnimation::animationWithKeyPath(Some(ns_string!("transform")));
                        let values = [start, 1.1, 1.0].map(|scale| {
                            // SAFETY: NSValue boxes the native transform struct.
                            unsafe { NSValue::valueWithCATransform3D(centered_scale(layer.bounds(), layer.anchorPoint(), scale)) }
                        });
                        let values: Vec<&AnyObject> = values.iter().map(|value| value.as_ref() as &AnyObject).collect();
                        // SAFETY: the transform key path interpolates boxed transforms.
                        unsafe { animation.setValues(Some(&NSArray::from_slice(&values))) };
                        let times = [0.0, 0.6, 1.0].map(NSNumber::new_f64);
                        animation.setKeyTimes(Some(&NSArray::from_slice(&[&*times[0], &*times[1], &*times[2]])));
                        let out = CAMediaTimingFunction::functionWithName(unsafe { kCAMediaTimingFunctionEaseOut });
                        let settle = CAMediaTimingFunction::functionWithName(unsafe { kCAMediaTimingFunctionEaseInEaseOut });
                        animation.setTimingFunctions(Some(&NSArray::from_slice(&[&*out, &*settle])));
                        animation.setDuration(seconds);
                        layer.addAnimation_forKey(&animation, Some(ns_string!(MOTION_KEY)));
                    }
                }
            }
            CATransaction::commit();
        }
    }
);

impl OverlayMotionView {
    pub(super) fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(MotionIvars {
            scale: Cell::new(1.0),
            pending: Cell::new(None),
        });
        // SAFETY: initWithFrame: is NSView's designated initializer.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this.setWantsLayer(true);
        this.setLayerContentsRedrawPolicy(NSViewLayerContentsRedrawPolicy::OnSetNeedsDisplay);
        this
    }

    pub(super) fn scale(&self) -> f64 {
        self.ivars().scale.get()
    }

    pub(super) fn set_scale(&self, scale: f64, seconds: f64) {
        if self.ivars().scale.replace(scale) == scale && seconds > 0.0 {
            return;
        }
        self.ivars().pending.set(Some(if seconds > 0.0 {
            Motion::Scale { seconds }
        } else {
            Motion::Set
        }));
        self.setNeedsDisplay(true);
        self.displayIfNeeded();
    }

    pub(super) fn pop_in(&self, start: f64, seconds: f64) {
        self.ivars().scale.set(1.0);
        self.ivars()
            .pending
            .set(Some(Motion::Pop { seconds, start }));
        self.setNeedsDisplay(true);
        self.displayIfNeeded();
    }
}
