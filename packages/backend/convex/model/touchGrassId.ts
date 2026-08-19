export const TOUCH_GRASS_ID_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

const TOUCH_GRASS_ID_PATTERN = /^TG-[A-HJ-NP-Z2-9]{6}$/;

export function validTouchGrassId(value: string) {
  return TOUCH_GRASS_ID_PATTERN.test(value);
}
