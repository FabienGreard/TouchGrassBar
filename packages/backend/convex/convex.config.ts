import aggregate from "@convex-dev/aggregate/convex.config.js";
import betterAuth from "@convex-dev/better-auth/convex.config";
import migrations from "@convex-dev/migrations/convex.config.js";
import rateLimiter from "@convex-dev/rate-limiter/convex.config.js";
import { defineApp } from "convex/server";
import { v } from "convex/values";

const app = defineApp({
  env: {
    BETTER_AUTH_SECRET: v.string(),
  },
});
app.use(aggregate, { name: "doomerboard" });
app.use(betterAuth);
app.use(migrations);
app.use(rateLimiter);

export default app;
