package app.evoparseq.tv

import android.content.Context
import android.hardware.input.InputManager
import android.os.Handler
import android.os.Looper
import android.os.VibrationEffect
import android.view.InputDevice
import android.view.KeyEvent
import android.view.MotionEvent

/** OS events and device-local vibration only; state, slots and wire encoding are Rust. */
internal class ControllerInput(context: Context) : InputManager.InputDeviceListener {
    private val input = context.getSystemService(Context.INPUT_SERVICE) as InputManager
    private val main = Handler(Looper.getMainLooper())
    private val mappings = mutableMapOf<Int, Mapping>()
    private val vibrating = mutableMapOf<Int, android.os.Vibrator>()
    private var closed = false
    var enabled = false
        set(value) {
            if (field == value) return
            field = value
            NativeCore.controllerFocus(value)
            if (!value) stopMotors()
        }

    init {
        NativeCore.controllerAdapter(this)
        input.registerInputDeviceListener(this, main)
    }
    private fun controller(id: Int): InputDevice? = input.getInputDevice(id)?.takeIf {
        !it.isVirtual && ((it.sources and InputDevice.SOURCE_GAMEPAD) == InputDevice.SOURCE_GAMEPAD ||
            (it.sources and InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK)
    }
    fun key(event: KeyEvent): Boolean {
        if (!enabled) return false
        val device = controller(event.deviceId) ?: return false
        val code = event.keyCode
        if (!KeyEvent.isGamepadButton(code) && code !in KeyEvent.KEYCODE_DPAD_UP..KeyEvent.KEYCODE_DPAD_RIGHT) return false
        if (code == KeyEvent.KEYCODE_BUTTON_MODE) return false // Android-owned Guide/Home.
        if (event.isCanceled) { NativeCore.controllerRemoved(device.id); return true }
        if (event.repeatCount != 0) return true
        val m = mappings.getOrPut(device.id) { Mapping(device) }
        // Some devices emit BOTH an analog trigger and a digital key. Only use
        // the key as fallback when the device declares no corresponding axis.
        if ((code == KeyEvent.KEYCODE_BUTTON_L2 && m.has(m.lt)) ||
            (code == KeyEvent.KEYCODE_BUTTON_R2 && m.has(m.rt))) return true
        return if (event.action == KeyEvent.ACTION_DOWN || event.action == KeyEvent.ACTION_UP)
            NativeCore.controllerButton(device.id, code, event.action == KeyEvent.ACTION_DOWN)
        else false
    }
    private class Mapping(private val device: InputDevice) {
        fun has(axis: Int) = device.getMotionRange(axis, InputDevice.SOURCE_JOYSTICK) != null
        val rx = if (has(MotionEvent.AXIS_Z) && has(MotionEvent.AXIS_RZ)) MotionEvent.AXIS_Z else MotionEvent.AXIS_RX
        val ry = if (rx == MotionEvent.AXIS_Z) MotionEvent.AXIS_RZ else MotionEvent.AXIS_RY
        val lt = if (has(MotionEvent.AXIS_LTRIGGER)) MotionEvent.AXIS_LTRIGGER else MotionEvent.AXIS_BRAKE
        val rt = if (has(MotionEvent.AXIS_RTRIGGER)) MotionEvent.AXIS_RTRIGGER else MotionEvent.AXIS_GAS
        val axes = intArrayOf(MotionEvent.AXIS_X, MotionEvent.AXIS_Y, rx, ry, lt, rt)
        val flat = FloatArray(6) { device.getMotionRange(axes[it], InputDevice.SOURCE_JOYSTICK)?.flat ?: 0f }
    }
    fun motion(event: MotionEvent): Boolean {
        if (!enabled || !event.isFromSource(InputDevice.SOURCE_JOYSTICK)) return false
        val device = controller(event.deviceId) ?: return false
        val m = mappings.getOrPut(device.id) { Mapping(device) }
        NativeCore.controllerAxes(device.id, event.getAxisValue(m.axes[0]), event.getAxisValue(m.axes[1]),
            event.getAxisValue(m.axes[2]), event.getAxisValue(m.axes[3]), event.getAxisValue(m.axes[4]), event.getAxisValue(m.axes[5]),
            event.getAxisValue(MotionEvent.AXIS_HAT_X), event.getAxisValue(MotionEvent.AXIS_HAT_Y),
            m.flat[0], m.flat[1], m.flat[2], m.flat[3], m.flat[4], m.flat[5])
        return true
    }
    @androidx.annotation.Keep
    @Suppress("DEPRECATION")
    fun rumble(id: Int, low: Int, high: Int) {
        main.post {
            if (closed) return@post
            if (!enabled || (low == 0 && high == 0)) {
                vibrating.remove(id)?.let { runCatching { it.cancel() } }
                return@post
            }
            val vibrator = vibrating[id] ?: input.getInputDevice(id)?.vibrator ?: return@post
            if (vibrator.hasVibrator()) {
                // Keep this exact OS handle through hotplug/focus loss so its
                // cancellation token remains the one that started the effect.
                // Android does not promise low/high motor identities.
                val amplitude = if (vibrator.hasAmplitudeControl()) maxOf(low,high).coerceIn(1,255) else VibrationEffect.DEFAULT_AMPLITUDE
                if (runCatching { vibrator.vibrate(VibrationEffect.createWaveform(longArrayOf(0,1000), intArrayOf(0,amplitude),0)) }.isSuccess)
                    vibrating[id] = vibrator
            }
        }
    }
    @Suppress("DEPRECATION")
    private fun stopMotors() {
        vibrating.values.forEach { runCatching { it.cancel() } }
        vibrating.clear()
    }
    override fun onInputDeviceAdded(id: Int) = Unit // First face button claims the shared slot.
    override fun onInputDeviceChanged(id: Int) { onInputDeviceRemoved(id) }
    @Suppress("DEPRECATION")
    override fun onInputDeviceRemoved(id: Int) {
        vibrating.remove(id)?.let { runCatching { it.cancel() } }
        mappings.remove(id)
        NativeCore.controllerRemoved(id)
    }
    fun close() {
        enabled = false
        stopMotors()
        closed = true
        input.unregisterInputDeviceListener(this)
        NativeCore.controllerAdapter(null)
    }
}
