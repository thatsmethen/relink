import CoreAudio

/// The C callbacks consume dense native-endian Float32 frames. Do not accept a
/// format merely because it sets the float bit: byte strides are part of it.
func isSupportedAudioStream(_ format: AudioStreamBasicDescription, deviceRate: Double) -> Bool {
    let allowed: AudioFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked | kAudioFormatFlagIsNonInterleaved
    guard deviceRate.isFinite, deviceRate >= 8000, format.mSampleRate.isFinite,
          abs(format.mSampleRate - deviceRate) < 0.01,
          format.mFormatID == kAudioFormatLinearPCM,
          format.mFormatFlags & (kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked) == (kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked),
          format.mFormatFlags & ~allowed == 0,
          format.mBitsPerChannel == 32, format.mChannelsPerFrame > 0,
          format.mFramesPerPacket == 1 else { return false }
    let channels = format.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0 ? 1 : format.mChannelsPerFrame
    let (stride, overflow) = channels.multipliedReportingOverflow(by: 4)
    return !overflow && format.mBytesPerFrame == stride && format.mBytesPerPacket == stride
}
