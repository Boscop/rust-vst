#![warn(missing_docs)]

//! rust-vst2 is a rust implementation of the VST2.4 API
//!
//! # Plugins
//! All Plugins must implement the `Vst` trait and `std::default::Default`. The `vst_main!` macro
//! must also be called in order to export the necessary functions for the VST to function.
//!
//! ## `Vst` Trait
//! All methods in this trait have a default implementation except for the `get_info` method which
//! must be implemented by the Vst object. Any of the default implementations may be overriden for
//! custom functionality; the defaults do nothing on their own.
//!
//! ## `vst_main!` macro
//! `vst_main!` will export the necessary functions to create a proper VST. This must be called
//! with your VST struct name in order for the vst to work.
//!
//! ## Example plugin
//! A barebones VST plugin:
//!
//! ```no_run
//! #[macro_use]
//! extern crate vst2;
//!
//! use vst2::{Vst, Info};
//!
//! #[derive(Default)]
//! struct BasicVst;
//!
//! impl Vst for BasicVst {
//!     fn get_info(&self) -> Info {
//!         Info {
//!             name: "BasicVst".to_string(),
//!             unique_id: 1357, // Used by hosts to differentiate between plugins.
//!
//!             ..Default::default()
//!         }
//!     }
//! }
//!
//! vst_main!(BasicVst); //Important!
//! # fn main() {} //no_run
//! ```
//!
//! # Hosts
//! Hosts are currently not supported. TODO

extern crate libc;
extern crate num;
#[macro_use] extern crate log;
#[macro_use] extern crate bitflags;

use std::{ptr, mem};
use std::iter::IntoIterator;

use libc::c_void;

#[macro_use] pub mod enums; // Use `impl_clike!`
pub mod buffer;
pub mod api;
pub mod editor;
pub mod channels;
pub mod host;
pub mod plugin;
mod interfaces;

use enums::flags::plugin::*;
use enums::{CanDo, Supported};
use api::{HostCallback, AEffect};
use editor::Editor;
use channels::ChannelInfo;
use host::Host;

pub use plugin::Info;
pub use buffer::AudioBuffer;

/// VST plugins are identified by a magic number. This corresponds to 0x56737450.
pub const VST_MAGIC: i32 = ('V' as i32) << 24 |
                           ('s' as i32) << 16 |
                           ('t' as i32) << 8  |
                           ('P' as i32) << 0  ;

/// Exports the necessary symbols for the plugin to be used by a vst host.
///
/// This macro takes a type which must implement the traits `Vst` and `std::default::Default`.
#[macro_export]
macro_rules! vst_main {
    ($t:ty) => {
        #[cfg(target_os = "macos")]
        #[no_mangle]
        pub extern "system" fn main_macho(callback: $crate::api::HostCallback) -> *mut $crate::api::AEffect {
            VSTPluginMain(callback)
        }

        #[cfg(target_os = "windows")]
        #[allow(non_snake_case)]
        #[no_mangle]
        pub extern "system" fn MAIN(callback: $crate::api::HostCallback) -> *mut $crate::api::AEffect {
            VSTPluginMain(callback)
        }

        #[allow(non_snake_case)]
        #[no_mangle]
        pub extern "system" fn VSTPluginMain(callback: $crate::api::HostCallback) -> *mut $crate::api::AEffect {
            $crate::main::<$t>(callback)
        }
    }
}

/// Initializes a VST plugin and returns a raw pointer to an AEffect struct.
#[doc(hidden)]
pub fn main<T: Vst + Default>(callback: HostCallback) -> *mut AEffect {
    // Create a Box containing a zeroed AEffect. This is transmuted into a *mut pointer so that it
    // can be passed into the Host `wrap` method. The AEffect is then updated after the vst object
    // is created so that the host still contains a raw pointer to the AEffect struct.
    let effect = unsafe { mem::transmute(Box::new(mem::zeroed::<AEffect>())) };

    let host = Host::wrap(callback, effect);
    if host.vst_version() == 0 { // TODO: Better criteria would probably be useful here...
        return ptr::null_mut();
    }

    trace!("Creating VST plugin instance...");
    let mut vst = <T>::new(host);
    let info = vst.get_info().clone();

    // Update AEffect in place
    unsafe { *effect = AEffect {
        magic: VST_MAGIC,
        dispatcher: interfaces::dispatch, // fn pointer

        _process: interfaces::process_deprecated, // fn pointer

        setParameter: interfaces::set_parameter, // fn pointer
        getParameter: interfaces::get_parameter, // fn pointer

        numPrograms: info.presets,
        numParams: info.parameters,
        numInputs: info.inputs,
        numOutputs: info.outputs,

        flags: {
            let mut flag = CAN_REPLACING;

            if info.f64_precision {
                flag = flag | CAN_DOUBLE_REPLACING;
            }

            if vst.get_editor().is_some() {
                flag = flag | HAS_EDITOR;
            }

            if info.preset_chunks {
                flag = flag | PROGRAM_CHUNKS;
            }

            if let plugin::Category::Synth = info.category {
                flag = flag | IS_SYNTH;
            }

            flag.bits()
        },

        reserved1: 0,
        reserved2: 0,

        initialDelay: info.initial_delay,

        _realQualities: 0,
        _offQualities: 0,
        _ioRatio: 0.0,

        object: mem::transmute(Box::new(Box::new(vst) as Box<Vst>)),
        user: ptr::null_mut(),

        uniqueId: info.unique_id,
        version: info.version,

        processReplacing: interfaces::process_replacing, // fn pointer
        processReplacingF64: interfaces::process_replacing_f64, //fn pointer

        future: [0u8; 56]
    }};
    effect
}

/// Must be implemented by all VST plugins.
///
/// All methods except `get_info` provide a default implementation which does nothing and can be
/// safely overridden.
#[allow(unused_variables)]
pub trait Vst {
    /// This method must return an `Info` struct.
    fn get_info(&self) -> Info;

    /// Called during initialization to pass a Host wrapper to the plugin.
    ///
    /// This method can be overriden to set `host` as a field in the plugin struct.
    ///
    /// # Example
    ///
    /// ```
    /// // ...
    /// # extern crate vst2;
    /// # #[macro_use] extern crate log;
    /// # use vst2::{Vst, Info};
    /// use vst2::host::Host;
    ///
    /// # #[derive(Default)]
    /// struct Plugin {
    ///     host: Host
    /// }
    ///
    /// impl Vst for Plugin {
    ///     fn new(host: Host) -> Plugin {
    ///         Plugin {
    ///             host: host
    ///         }
    ///     }
    ///
    ///     fn init(&mut self) {
    ///         info!("loaded with host vst version: {}", self.host.vst_version());
    ///     }
    ///
    ///     // ...
    /// #     fn get_info(&self) -> Info {
    /// #         Info {
    /// #             name: "Example Plugin".to_string(),
    /// #             ..Default::default()
    /// #         }
    /// #     }
    /// }
    ///
    /// # fn main() {}
    /// ```
    fn new(host: Host) -> Self where Self: Sized + Default {
        Default::default()
    }

    /// Called when VST is fully initialized.
    fn init(&mut self) { trace!("Initialized vst plugin."); }


    /// Set the current preset to the index specified by `preset`.
    fn change_preset(&mut self, preset: i32) { }

    /// Get the current preset index.
    fn get_preset_num(&self) -> i32 { 0 }

    /// Set the current preset name.
    fn set_preset_name(&self, name: String) { }

    /// Get the name of the preset at the index specified by `preset`.
    fn get_preset_name(&self, preset: i32) -> String { "".to_string() }


    /// Get parameter label for parameter at `index` (e.g. "db", "sec", "ms", "%").
    fn get_parameter_label(&self, index: i32) -> String { "".to_string() }

    /// Get the parameter value for parameter at `index` (e.g. "1.0", "150", "Plate", "Off").
    fn get_parameter_text(&self, index: i32) -> String {
        format!("{:.3}", self.get_parameter(index))
    }

    /// Get the name of parameter at `index`.
    fn get_parameter_name(&self, index: i32) -> String { format!("Param {}", index) }

    /// Get the value of paramater at `index`. Should be value between 0.0 and 1.0.
    fn get_parameter(&self, index: i32) -> f32 { 0.0 }

    /// Set the value of parameter at `index`. `value` is between 0.0 and 1.0.
    fn set_parameter(&mut self, index: i32, value: f32) { }

    /// Return whether parameter at `index` can be automated.
    fn can_be_automated(&self, index: i32) -> bool { false }

    /// Use String as input for parameter value. Used by host to provide an editable field to
    /// adjust a parameter value. E.g. "100" may be interpreted as 100hz for parameter. Returns if
    /// the input string was used.
    fn string_to_parameter(&self, index: i32, text: String) -> bool { false }


    /// Called when sample rate is changed by host.
    fn sample_rate_changed(&mut self, rate: f32) { }

    /// Called when block size is changed by host.
    fn block_size_changed(&mut self, size: i64) { }


    /// Called when plugin is turned on.
    fn on_resume(&mut self) { }

    /// Called when plugin is turned off.
    fn on_suspend(&mut self) { }


    /// Vendor specific handling.
    fn vendor_specific(&mut self, index: i32, value: isize, ptr: *mut c_void, opt: f32) { }


    /// Return whether plugin supports specified action.
    fn can_do(&self, can_do: CanDo) -> Supported {
        info!("Host is asking if plugin can: {:?}.", can_do);
        Supported::Maybe
    }

    /// Get the tail size of plugin when it is stopped. Used in offline processing as well.
    fn get_tail_size(&self) -> isize { 0 }


    /// Process an audio buffer containing `f32` values. TODO: Examples
    fn process(&mut self, buffer: AudioBuffer<f32>) {
        // For each input and output
        for (input, output) in buffer.zip() {
            // For each input sample and output sample in buffer
            for (in_frame, out_frame) in input.into_iter().zip(output.into_iter()) {
                *out_frame = *in_frame;
            }
        }
    }

    /// Process an audio buffer containing `f64` values. TODO: Examples
    fn process_f64(&mut self, buffer: AudioBuffer<f64>) {
        // For each input and output
        for (input, output) in buffer.zip() {
            // For each input sample and output sample in buffer
            for (in_frame, out_frame) in input.into_iter().zip(output.into_iter()) {
                *out_frame = *in_frame;
            }
        }
    }

    /// Return handle to plugin editor if supported.
    fn get_editor(&mut self) -> Option<&mut Editor> { None }


    /// If `preset_chunks` is set to true in plugin info, this should return the raw chunk data for
    /// the current preset.
    fn get_preset_data(&mut self) -> Vec<u8> { Vec::new() }

    /// If `preset_chunks` is set to true in plugin info, this should return the raw chunk data for
    /// the current plugin bank.
    fn get_bank_data(&mut self) -> Vec<u8> { Vec::new() }

    /// If `preset_chunks` is set to true in plugin info, this should load a preset from the given
    /// chunk data.
    fn load_preset_data(&mut self, data: Vec<u8>) {}

    /// If `preset_chunks` is set to true in plugin info, this should load a preset bank from the
    /// given chunk data.
    fn load_bank_data(&mut self, data: Vec<u8>) {}

    /// Get information about an input channel. Only used by some hosts.
    fn get_input_info(&self, input: i32) -> ChannelInfo {
        ChannelInfo::new(format!("Input channel {}", input),
                         Some(format!("In {}", input)),
                         true, None)
    }

    /// Get information about an output channel. Only used by some hosts.
    fn get_output_info(&self, output: i32) -> ChannelInfo {
        ChannelInfo::new(format!("Output channel {}", output),
                         Some(format!("Out {}", output)),
                         true, None)
    }
}


#[cfg(test)]
#[allow(private_no_mangle_fns)] // For `vst_main!`
mod tests {
    use std::default::Default;
    use std::{mem, ptr};

    use libc::c_void;

    use Vst;
    use VST_MAGIC;
    use interfaces;
    use api::AEffect;
    use plugin::Info;

    #[derive(Default)]
    struct TestPlugin;

    impl Vst for TestPlugin {
        fn get_info(&self) -> Info {
            Info {
                name: "Test Plugin".to_string(),
                vendor: "overdrivenpotato".to_string(),

                presets: 1,
                parameters: 1,

                unique_id: 5678,
                version: 1234,

                initial_delay: 123,

                ..Default::default()
            }
        }
    }

    vst_main!(TestPlugin);

    fn pass_callback(_effect: *mut AEffect, _opcode: i32, _index: i32, _value: isize, _ptr: *mut c_void, _opt: f32) -> isize {
        1
    }

    fn fail_callback(_effect: *mut AEffect, _opcode: i32, _index: i32, _value: isize, _ptr: *mut c_void, _opt: f32) -> isize {
        0
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn old_hosts() {
        assert_eq!(MAIN(fail_callback), ptr::null_mut());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn old_hosts() {
        assert_eq!(main_macho(fail_callback), ptr::null_mut());
    }

    #[test]
    fn host_callback() {
        assert_eq!(VSTPluginMain(fail_callback), ptr::null_mut());
    }

    #[test]
    fn aeffect_created() {
        let aeffect = VSTPluginMain(pass_callback);
        assert!(!aeffect.is_null());
    }

    #[test]
    fn vst_drop() {
        static mut drop_test: bool = false;

        impl Drop for TestPlugin {
            fn drop(&mut self) {
                unsafe { drop_test = true; }
            }
        }

        let aeffect = VSTPluginMain(pass_callback);
        assert!(!aeffect.is_null());

        unsafe { (*aeffect).drop_vst() };

        // Assert that the VST is shut down and dropped.
        assert!(unsafe { drop_test });
    }

    #[test]
    fn vst_no_drop() {
        let aeffect = VSTPluginMain(pass_callback);
        assert!(!aeffect.is_null());

        // Make sure this doesn't crash.
        unsafe { (*aeffect).drop_vst() };
    }

    #[test]
    fn vst_deref() {
        let aeffect = VSTPluginMain(pass_callback);
        assert!(!aeffect.is_null());

        let vst = unsafe { (*aeffect).get_vst() };
        // Assert that deref works correctly.
        assert!(vst.get_info().name == "Test Plugin");
    }

    #[test]
    fn aeffect_params() {
        // Assert that 2 function pointers are equal.
        macro_rules! assert_fn_eq {
            ($a:expr, $b:expr) => {
                unsafe {
                    assert_eq!(
                        mem::transmute::<_, usize>($a),
                        mem::transmute::<_, usize>($b)
                    );
                }
            }
        }

        let aeffect = unsafe { &mut *VSTPluginMain(pass_callback) };

        assert_eq!(aeffect.magic, VST_MAGIC);
        assert_fn_eq!(aeffect.dispatcher, interfaces::dispatch);
        assert_fn_eq!(aeffect._process, interfaces::process_deprecated);
        assert_fn_eq!(aeffect.setParameter, interfaces::set_parameter);
        assert_fn_eq!(aeffect.getParameter, interfaces::get_parameter);
        assert_eq!(aeffect.numPrograms, 1);
        assert_eq!(aeffect.numParams, 1);
        assert_eq!(aeffect.numInputs, 2);
        assert_eq!(aeffect.numOutputs, 2);
        assert_eq!(aeffect.reserved1, 0);
        assert_eq!(aeffect.reserved2, 0);
        assert_eq!(aeffect.initialDelay, 123);
        assert_eq!(aeffect.uniqueId, 5678);
        assert_eq!(aeffect.version, 1234);
        assert_fn_eq!(aeffect.processReplacing, interfaces::process_replacing);
        assert_fn_eq!(aeffect.processReplacingF64, interfaces::process_replacing_f64);
    }
}
