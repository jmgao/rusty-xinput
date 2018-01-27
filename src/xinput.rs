//! XInput stuff. DOCS TODO.

use std::ffi::CString;

use winapi::shared::minwindef::{DWORD, HMODULE};
use winapi::shared::winerror::{ERROR_DEVICE_NOT_CONNECTED, ERROR_SUCCESS};
use winapi::um::libloaderapi::{FreeLibrary, GetProcAddress, LoadLibraryW};
use winapi::um::xinput::*;

type XInputGetStateFunc = unsafe extern "system" fn(DWORD, *mut XINPUT_STATE) -> DWORD;
type XInputSetStateFunc = unsafe extern "system" fn(DWORD, *mut XINPUT_VIBRATION) -> DWORD;

static mut global_xinput_handle: HMODULE = ::std::ptr::null_mut();
static mut opt_xinput_get_state: Option<XInputGetStateFunc> = None;
static mut opt_xinput_set_state: Option<XInputSetStateFunc> = None;
static xinput_status: ::std::sync::atomic::AtomicUsize = ::std::sync::atomic::ATOMIC_USIZE_INIT;
const ordering: ::std::sync::atomic::Ordering = ::std::sync::atomic::Ordering::SeqCst;

const xinput_UNINITIALIZED: usize = 0;
const xinput_LOADING: usize = 1;
const xinput_ACTIVE: usize = 2;

fn wide_null<S: AsRef<str>>(s: S) -> Vec<u16> {
  let mut output = vec![];
  for u in s.as_ref().encode_utf16() {
    output.push(u)
  }
  output.push(0);
  output
}

fn show_wide_null(arr: &[u16]) -> String {
  arr
    .iter()
    .take_while(|&&u| u != 0)
    .map(|&u| u as u8 as char)
    .collect()
}

/// The ways that a dynamic load of XInput can fail.
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub enum XInputLoadingFailure {
  /// The xinput system was already in the process of loading in some other
  /// thread. This attempt failed because of that, but that other attempt might
  /// still succeed.
  AlreadyLoading,
  /// The xinput system was already active. A failure of this kind leaves the
  /// system active.
  AlreadyActive,
  /// The system was not loading or active, but was in some unknown state. If
  /// you get this, it's probably a bug that you should report.
  UnknownState,
  /// No DLL for XInput could be found. This places the system back into an
  /// "uninitialized" status, and you could potentially try again later if the
  /// user fiddles with the program's DLL path or whatever.
  NoDLL,
  /// A DLL was found that matches one of the expected XInput DLL names, but it
  /// didn't contain both of the expected functions. This is probably a weird
  /// situation to find. Either way, the xinput status is set to "uninitialized"
  /// and as with the NoDLL error you could potentially try again.
  NoPointers,
}

/// Attempts to dynamically load an XInput DLL and get the function pointers.
///
/// This operation is thread-safe and can be performed at any time. If xinput
/// hasn't been loaded yet, or if there was a failed load attempt, then
/// `xinput_get_state` and `xinput_set_state` will safety return `None`.
///
/// There's no way to unload XInput once it's been loaded, because it makes the
/// normal operation a little faster, and why would you want to unload it anyway?
///
/// # Failure
///
/// This can fail in a few ways, as explained in the `XInputLoadingFailure`
/// type. The most likely failure case is that the user's system won't have the
/// required DLL, in which case you should probably allow them to play with just
/// a keyboard/mouse instead.
pub fn dynamic_load_xinput() -> Result<(), XInputLoadingFailure> {
  // The result status is if the value was what we expected, and the value
  // inside is actual value seen.
  match xinput_status.compare_exchange(xinput_UNINITIALIZED, xinput_LOADING, ordering, ordering) {
    Err(xinput_LOADING) => {
      debug!("A call to 'dynamic_load_xinput' was made while XInput was already loading.");
      Err(XInputLoadingFailure::AlreadyLoading)
    }
    Err(xinput_ACTIVE) => {
      debug!("A call to 'dynamic_load_xinput' was made while XInput was already active.");
      Err(XInputLoadingFailure::AlreadyActive)
    }
    Err(_) => {
      warn!("A call to 'dynamic_load_xinput' was made while XInput was in an unknown state.");
      Err(XInputLoadingFailure::UnknownState)
    }
    Ok(_) => {
      let xinput14 = wide_null("xinput1_4.dll");
      let xinput91 = wide_null("xinput9_1_0.dll");
      let xinput13 = wide_null("xinput1_3.dll");

      let mut xinput_handle: HMODULE = ::std::ptr::null_mut();
      for lib_name in vec![xinput14, xinput91, xinput13] {
        trace!(
          "Attempting to load XInput DLL: {}",
          show_wide_null(&lib_name)
        );
        // It's safe to call this, the worst that can happen is that we get a null back.
        xinput_handle = unsafe { LoadLibraryW(lib_name.as_ptr()) };
        if !xinput_handle.is_null() {
          debug!("Success: XInput Loaded: {}", show_wide_null(&lib_name));
          break;
        }
      }
      if xinput_handle.is_null() {
        debug!("Failure: XInput could not be loaded.");
        xinput_status
          .compare_exchange(xinput_LOADING, xinput_UNINITIALIZED, ordering, ordering)
          .ok();
        Err(XInputLoadingFailure::NoDLL)
      } else {
        let get_state_name = CString::new("XInputGetState").unwrap();
        let set_state_name = CString::new("XInputSetState").unwrap();

        // using transmute is so dodgy we'll put that in its own unsafe block.
        unsafe {
          let get_state_ptr = GetProcAddress(xinput_handle, get_state_name.as_ptr());
          if !get_state_ptr.is_null() {
            trace!("Found function {:?}.", get_state_name);
            opt_xinput_get_state = Some(::std::mem::transmute(get_state_ptr));
          } else {
            trace!("Could not find function {:?}.", get_state_name);
          }
        }

        // using transmute is so dodgy we'll put that in its own unsafe block.
        unsafe {
          let set_state_ptr = GetProcAddress(xinput_handle, set_state_name.as_ptr());
          if !set_state_ptr.is_null() {
            trace!("Found Function {:?}.", set_state_name);
            opt_xinput_set_state = Some(::std::mem::transmute(set_state_ptr));
          } else {
            trace!("Could not find function {:?}.", set_state_name);
          }
        }

        // this is safe because no other code can be loading xinput at the same time as us.
        unsafe {
          if opt_xinput_get_state.is_some() && opt_xinput_set_state.is_some() {
            global_xinput_handle = xinput_handle;
            debug!("Function pointers loaded successfully.");
            xinput_status
              .compare_exchange(xinput_LOADING, xinput_ACTIVE, ordering, ordering)
              .ok();
            Ok(())
          } else {
            opt_xinput_get_state = None;
            opt_xinput_set_state = None;
            FreeLibrary(xinput_handle);
            debug!("Could not load the function pointers.");
            xinput_status
              .compare_exchange(xinput_LOADING, xinput_UNINITIALIZED, ordering, ordering)
              .ok();
            Err(XInputLoadingFailure::NoPointers)
          }
        }
      }
    }
  }
}

/// This wraps an `XINPUT_STATE` value and provides a more rusty (read-only)
/// interface to the data it contains.
///
/// All three major game companies use different names for most of the buttons,
/// so the docs for each button method list out what each of the major companies
/// call that button. To the driver it's all the same, it's just however you
/// want to think of them.
///
/// If sequential calls to `xinput_get_state` for a given controller slot have
/// the same packet number then the controller state has not changed since the
/// last call. The `PartialEq` and `Eq` implementations for this wrapper type
/// reflect that. The exact value of the packet number is unimportant.
///
/// If you want to do something that the rust wrapper doesn't support, just use
/// the raw field to get at the inner value.
pub struct XInputState {
  /// The raw value we're wrapping.
  pub raw: XINPUT_STATE,
}

impl ::std::cmp::PartialEq for XInputState {
  /// Equality for `XInputState` values is based _only_ on the
  /// `dwPacketNumber` of the wrapped `XINPUT_STATE` value. This is entirely
  /// correct for values obtained from the xinput system, but if you make your
  /// own `XInputState` values for some reason you can confuse it.
  fn eq(&self, other: &XInputState) -> bool {
    self.raw.dwPacketNumber == other.raw.dwPacketNumber
  }
}

impl ::std::cmp::Eq for XInputState {}

impl ::std::fmt::Debug for XInputState {
  fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
    write!(f, "XInputState (_)")
  }
}

impl XInputState {
  /// The north button of the action button group.
  ///
  /// * Nintendo: X
  /// * Playstation: Triangle
  /// * XBox: Y
  pub fn north_button(&self) -> bool {
    self.raw.Gamepad.wButtons & XINPUT_GAMEPAD_Y != 0
  }

  /// The south button of the action button group.
  ///
  /// * Nintendo: B
  /// * Playstation: X
  /// * XBox: A
  pub fn south_button(&self) -> bool {
    self.raw.Gamepad.wButtons & XINPUT_GAMEPAD_A != 0
  }

  /// The east button of the action button group.
  ///
  /// * Nintendo: A
  /// * Playstation: Circle
  /// * XBox: B
  pub fn east_button(&self) -> bool {
    self.raw.Gamepad.wButtons & XINPUT_GAMEPAD_B != 0
  }

  /// The west button of the action button group.
  ///
  /// * Nintendo: Y
  /// * Playstation: Square
  /// * XBox: X
  pub fn west_button(&self) -> bool {
    self.raw.Gamepad.wButtons & XINPUT_GAMEPAD_X != 0
  }

  /// The up button on the directional pad.
  pub fn arrow_up(&self) -> bool {
    self.raw.Gamepad.wButtons & XINPUT_GAMEPAD_DPAD_UP != 0
  }

  /// The down button on the directional pad.
  pub fn arrow_down(&self) -> bool {
    self.raw.Gamepad.wButtons & XINPUT_GAMEPAD_DPAD_DOWN != 0
  }

  /// The left button on the directional pad.
  pub fn arrow_left(&self) -> bool {
    self.raw.Gamepad.wButtons & XINPUT_GAMEPAD_DPAD_LEFT != 0
  }

  /// The right button on the directional pad.
  pub fn arrow_right(&self) -> bool {
    self.raw.Gamepad.wButtons & XINPUT_GAMEPAD_DPAD_RIGHT != 0
  }

  /// The "start" button.
  ///
  /// * Nintendo: Start (NES / NES), '+' (Pro Controller)
  /// * Playstation: Start
  /// * XBox: Start
  pub fn start_button(&self) -> bool {
    self.raw.Gamepad.wButtons & XINPUT_GAMEPAD_START != 0
  }

  /// The "not start" button.
  ///
  /// * Nintendo: Select (NES / NES), '-' (Pro Controller)
  /// * Playstation: Select
  /// * XBox: Back
  pub fn select_button(&self) -> bool {
    self.raw.Gamepad.wButtons & XINPUT_GAMEPAD_BACK != 0
  }

  /// The upper left shoulder button.
  ///
  /// * Nintendo: L
  /// * Playstation: L1
  /// * XBox: LB
  pub fn left_shoulder(&self) -> bool {
    self.raw.Gamepad.wButtons & XINPUT_GAMEPAD_RIGHT_SHOULDER != 0
  }

  /// The upper right shoulder button.
  ///
  /// * Nintendo: R
  /// * Playstation: R1
  /// * XBox: RB
  pub fn right_shoulder(&self) -> bool {
    self.raw.Gamepad.wButtons & XINPUT_GAMEPAD_RIGHT_SHOULDER != 0
  }

  /// The default threshold to count a trigger as being "pressed".
  pub const TRIGGER_THRESHOLD: u8 = XINPUT_GAMEPAD_TRIGGER_THRESHOLD;

  /// The lower left shoulder trigger. If you want to use this as a simple
  /// boolean it is suggested that you compare it to the `TRIGGER_THRESHOLD`
  /// constant.
  ///
  /// * Nintendo: ZL
  /// * Playstation: L2
  /// * XBox: LT
  pub fn left_trigger(&self) -> u8 {
    self.raw.Gamepad.bLeftTrigger
  }

  /// The lower right shoulder trigger. If you want to use this as a simple
  /// boolean it is suggested that you compare it to the `TRIGGER_THRESHOLD`
  /// constant.
  ///
  /// * Nintendo: ZR
  /// * Playstation: R2
  /// * XBox: RT
  pub fn right_trigger(&self) -> u8 {
    self.raw.Gamepad.bRightTrigger
  }

  /// The lower left shoulder trigger as a bool using the default threshold.
  ///
  /// * Nintendo: ZL
  /// * Playstation: L2
  /// * XBox: LT
  pub fn left_trigger_bool(&self) -> bool {
    self.left_trigger() >= XInputState::TRIGGER_THRESHOLD
  }

  /// The lower right shoulder trigger as a bool using the default threshold.
  ///
  /// * Nintendo: ZR
  /// * Playstation: R2
  /// * XBox: RT
  pub fn right_trigger_bool(&self) -> bool {
    self.right_trigger() >= XInputState::TRIGGER_THRESHOLD
  }

  /// The left thumb stick being pressed inward.
  ///
  /// * Nintendo: (L)
  /// * Playstation: L3
  /// * XBox: (L)
  pub fn left_thumb_button(&self) -> bool {
    self.raw.Gamepad.wButtons & XINPUT_GAMEPAD_LEFT_THUMB != 0
  }

  /// The right thumb stick being pressed inward.
  ///
  /// * Nintendo: (R)
  /// * Playstation: R3
  /// * XBox: (R)
  pub fn right_thumb_button(&self) -> bool {
    self.raw.Gamepad.wButtons & XINPUT_GAMEPAD_RIGHT_THUMB != 0
  }

  /// The suggested default deadzone for use with the left thumb stick.
  pub const LEFT_STICK_DEADZONE: i16 = XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE;

  /// The suggested default deadzone for use with the right thumb stick.
  pub const RIGHT_STICK_DEADZONE: i16 = XINPUT_GAMEPAD_RIGHT_THUMB_DEADZONE;

  /// The left stick raw value.
  ///
  /// Positive values are to the right (X-axis) or up (Y-axis).
  pub fn left_stick_raw(&self) -> (i16, i16) {
    (self.raw.Gamepad.sThumbLX, self.raw.Gamepad.sThumbLY)
  }

  /// The right stick raw value.
  ///
  /// Positive values are to the right (X-axis) or up (Y-axis).
  pub fn right_stick_raw(&self) -> (i16, i16) {
    (self.raw.Gamepad.sThumbRX, self.raw.Gamepad.sThumbRY)
  }

  /// The left stick value normalized with the default dead-zone.
  ///
  /// See `normalize_raw_stick_value` for more.
  pub fn left_stick_normalized(&self) -> (f32, f32) {
    XInputState::normalize_raw_stick_value(self.left_stick_raw(), XInputState::LEFT_STICK_DEADZONE)
  }

  /// The right stick value normalized with the default dead-zone.
  ///
  /// See `normalize_raw_stick_value` for more.
  pub fn right_stick_normalized(&self) -> (f32, f32) {
    XInputState::normalize_raw_stick_value(
      self.right_stick_raw(),
      XInputState::RIGHT_STICK_DEADZONE,
    )
  }

  /// This helper normalizes a raw stick value using the given deadzone.
  ///
  /// If the raw value's 2d length is less than the deadzone the result will be
  /// `(0.0,0.0)`, otherwise the result is normalized across the range from the
  /// deadzone point to the maximum value.
  ///
  /// The `deadzone` value is clamped to the range 0 to 32,766 (inclusive)
  /// before use. Negative inputs or maximum value inputs make the normalization
  /// just work improperly.
  pub fn normalize_raw_stick_value(raw_stick: (i16, i16), deadzone: i16) -> (f32, f32) {
    let deadzone_float = deadzone.max(0).min(i16::max_value() - 1) as f32;
    let raw_float = (raw_stick.0 as f32, raw_stick.1 as f32);
    let length = (raw_float.0 * raw_float.0 + raw_float.1 * raw_float.1).sqrt();
    let normalized = (raw_float.0 / length, raw_float.1 / length);
    if length > deadzone_float {
      // clip our value to the expected maximum length.
      let length = length.min(32_767.0);
      let scale = (length - deadzone_float) / (32_767.0 - deadzone_float);
      (normalized.0 * scale, normalized.1 * scale)
    } else {
      (0.0, 0.0)
    }
  }
}

#[test]
fn normalize_raw_stick_value_test() {
  for &x in [i16::min_value(), i16::max_value()].into_iter() {
    for &y in [i16::min_value(), i16::max_value()].into_iter() {
      #[cfg_attr(rustfmt, rustfmt_skip)]
      for &deadzone in [i16::min_value(), 0, i16::max_value() / 2,
                        i16::max_value() - 1, i16::max_value()].into_iter() {
        let f = XInputState::normalize_raw_stick_value((x, y), deadzone);
        #[cfg_attr(rustfmt, rustfmt_skip)]
        assert!(f.0.abs() <= 1.0, "XFail: x {}, y {}, dz {} f {:?}", x, y, deadzone, f);
        #[cfg_attr(rustfmt, rustfmt_skip)]
        assert!(f.1.abs() <= 1.0, "YFail: x {}, y {}, dz {} f {:?}", x, y, deadzone, f);
      }
    }
  }
}

/// Polls the controller port given for the current controller state.
///
/// # Failure
///
/// If the xinput system isn't ready, if there's no controller in that slot, or
/// if the slot if out of bounds, you simply get `None`.
pub fn xinput_get_state(user_index: u32) -> Option<XInputState> {
  if xinput_status.load(ordering) == xinput_ACTIVE && user_index < 4 {
    let mut output: XINPUT_STATE = unsafe { ::std::mem::zeroed() };
    let return_status = unsafe {
      // This unwrap is safe only because we don't currently support unloading
      // the system once it's active. Otherwise we'd have to use a full mutex
      // and all that.
      let func = opt_xinput_get_state.unwrap();
      func(user_index, &mut output)
    };
    match return_status {
      ERROR_SUCCESS => return Some(XInputState { raw: output }),
      ERROR_DEVICE_NOT_CONNECTED => return None,
      s => {
        trace!("Unexpected error code: {}", s);
        return None;
      }
    };
  } else {
    None
  }
}

/// Allows you to set the rumble speeds of the left and right motors.
///
/// Valid motor speeds are across the whole `u16` range, and the number is the
/// scale of the motor intensity. In other words, 0 is 0%, and 65,535 is 100%.
///
/// On a 360 controller the left motor is low-frequency and the right motor is
/// high-frequency. On other controllers running through xinput this might be
/// the case, or the controller might not even have rumble ability at all.
///
/// # Failure
///
/// If the xinput system isn't ready, if there's no controller in that slot, or
/// if the slot if out of bounds, you simply get `None`.
pub fn xinput_set_state(
  user_index: u32, left_motor_speed: u16, right_motor_speed: u16
) -> Option<()> {
  if xinput_status.load(ordering) == xinput_ACTIVE && user_index < 4 {
    let mut input = XINPUT_VIBRATION {
      wLeftMotorSpeed: left_motor_speed,
      wRightMotorSpeed: right_motor_speed,
    };
    let return_status = unsafe {
      // This unwrap is safe only because we don't currently support unloading
      // the system once it's active. Otherwise we'd have to use a full mutex
      // and all that.
      let func = opt_xinput_set_state.unwrap();
      func(user_index, &mut input)
    };
    match return_status {
      ERROR_SUCCESS => return Some(()),
      ERROR_DEVICE_NOT_CONNECTED => return None,
      s => {
        trace!("Unexpected error code: {}", s);
        return None;
      }
    };
  } else {
    None
  }
}