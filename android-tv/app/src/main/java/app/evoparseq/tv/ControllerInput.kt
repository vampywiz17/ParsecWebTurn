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
    private val supportedKeys = setOf(19,20,21,22,96,97,99,100,102,103,104,105,106,107,108,109)
    private val mappings = mutableMapOf<Int, Mapping>()
    private val vibrating = mutableSetOf<Int>()
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
        if (!enabled || controller(event.deviceId) == null) return false
        if (event.keyCode !in supportedKeys) return false
        if (event.isCanceled) NativeCore.controllerRemoved(event.deviceId)
        else if (event.repeatCount == 0 && (event.action == KeyEvent.ACTION_DOWN || event.action == KeyEvent.ACTION_UP))
            NativeCore.controllerButton(event.deviceId, event.keyCode, event.action == KeyEvent.ACTION_DOWN)
        return true
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
            val vibrator = input.getInputDevice(id)?.vibrator ?: return@post
            if (!enabled || (low == 0 && high == 0)) {
                if (vibrating.remove(id)) vibrator.cancel()
            } else if (vibrator.hasVibrator()) {
                // Android does not promise low/high motor identities. Apply the
                // combined amplitude to this controller only, never the TV/phone.
                vibrator.vibrate(VibrationEffect.createWaveform(longArrayOf(0,1000), intArrayOf(0,maxOf(low,high).coerceIn(1,255)),0))
                vibrating.add(id)
            }
        }
    }
    @Suppress("DEPRECATION")
    private fun stopMotors() {
        vibrating.forEach { input.getInputDevice(it)?.vibrator?.cancel() }
        vibrating.clear()
    }
    override fun onInputDeviceAdded(id: Int) = Unit // First face button claims the shared slot.
    override fun onInputDeviceChanged(id: Int) { onInputDeviceRemoved(id) }
    @Suppress("DEPRECATION")
    override fun onInputDeviceRemoved(id: Int) {
        input.getInputDevice(id)?.vibrator?.cancel()
        vibrating.remove(id)
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
