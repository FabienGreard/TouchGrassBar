# Keep sensitive provider data on device

TouchGrassBar keeps prompts, conversations, credentials, cookies, raw logs, and local file paths on the Tokenmaxxer's Mac. Daily Usage Aggregates may be synchronized to Convex, because social comparison requires shared totals but does not justify centralizing the sensitive source material used to calculate them.

[ADR-0021](0021-send-bounded-failure-diagnostics.md) also permits bounded,
structured operational failure reports for private support access. These
reports contain defined error codes and approved technical context. They do
not contain the sensitive source material listed above, and successful
operations send no report.
