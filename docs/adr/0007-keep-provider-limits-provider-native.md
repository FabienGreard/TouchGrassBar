# Keep provider limits provider-native

TouchGrassBar mirrors Codex and Claude limits using each provider's reported lanes, labels, units, remaining values, and reset times rather than replacing them with normalized limits. Provider limits answer availability questions, while Token Scores and API-Equivalent Cost answer separate historical and social questions.

Amendment for issue #41: The separate Overall Quota Headroom index reduces each provider's active lanes to a remaining share and then calculates an equal-weighted mean across enabled providers. This summary does not change or replace the provider-native limits.
