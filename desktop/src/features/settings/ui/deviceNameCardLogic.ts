import type { DeviceIdentity } from "@/shared/api/types";

/**
 * The anonymous default label for a device, mirroring the backend's
 * `opaque_label` in `device_identity.rs`: `device-` plus the first eight
 * characters of the non-rotating `device_id`.
 */
export function opaqueLabelFor(deviceId: string): string {
  return `device-${deviceId.slice(0, 8)}`;
}

/**
 * Whether the Reset control belongs on screen.
 *
 * Compared against *this* device's derived default rather than the
 * `device-xxxxxxxx` shape: an owner who typed a lookalike label still needs
 * the way back, and a device already sitting on its default must not be
 * offered a no-op.
 */
export function showsResetControl(
  identity: DeviceIdentity | undefined,
): boolean {
  if (!identity) return false;
  return identity.deviceLabel !== opaqueLabelFor(identity.deviceId);
}

/**
 * Whether Save is disabled.
 *
 * A reset in flight disables it too: both writes land on the same
 * `device.json` through `set_label_at` and both republish, so allowing the
 * pair concurrently makes the published label a race.
 */
export function saveIsDisabled(input: {
  trimmedLabel: string;
  savedLabel: string;
  identityLoaded: boolean;
  renamePending: boolean;
  resetPending: boolean;
}): boolean {
  return (
    input.trimmedLabel.length === 0 ||
    input.trimmedLabel === input.savedLabel ||
    input.renamePending ||
    input.resetPending ||
    !input.identityLoaded
  );
}
