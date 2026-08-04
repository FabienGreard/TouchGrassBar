function normalizeTouchGrassId(value: string) {
  return value.trim().replace(/^#/, "").toUpperCase();
}

export { normalizeTouchGrassId };
