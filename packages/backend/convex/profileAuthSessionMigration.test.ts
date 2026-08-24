/// <reference types="vite/client" />

import migrationsTest from "@convex-dev/migrations/test";
import { convexTest } from "convex-test";
import { expect, test } from "vitest";

import { internal } from "./_generated/api";
import schema from "./schema";

const modules = import.meta.glob("./**/*.ts");
const PROFILE_COUNT = 121;

function testBackend() {
  const t = convexTest(schema, modules);
  migrationsTest.register(t);
  return t;
}

async function seedProfiles(t: ReturnType<typeof testBackend>) {
  await t.run(async (ctx) => {
    for (let index = 0; index < PROFILE_COUNT; index += 1) {
      const generation = (index % 5) + 1;
      const activeAuthSessionIdMissing = index % 4 === 0 || index % 4 === 1;
      const authSessionGenerationMissing = index % 4 === 0 || index % 4 === 2;
      const tokenmaxxerId = await ctx.db.insert("tokenmaxxers", {
        ...(activeAuthSessionIdMissing
          ? {}
          : { activeAuthSessionId: index % 8 === 3 ? null : `session-${index}` }),
        ...(authSessionGenerationMissing ? {} : { authSessionGeneration: generation }),
        authSubject: `auth-subject-${index}`,
        createdAt: index,
        displayName: `Profile ${index}`,
        publicId: `TG-MIGRATION-${index}`,
      });
      const activeDeviceId = await ctx.db.insert("devices", {
        createdAt: index,
        generation,
        installationCredentialDigest: `digest-${index}`,
        lastSeenAt: index,
        tokenmaxxerId,
        usageBackfillCompletedAt: null,
      });
      await ctx.db.patch(tokenmaxxerId, { activeDeviceId });
    }
  });
}

async function migrationBatch(t: ReturnType<typeof testBackend>, cursor: string | null) {
  return await t.mutation(internal.internal.migrations.backfillProfileAuthSessionFence, {
    cursor,
  });
}

async function finishMigration(t: ReturnType<typeof testBackend>, cursor: string | null) {
  let nextCursor = cursor;
  let batches = 0;
  let changedProfiles = 0;
  let invalidActiveMacAuthorities = 0;
  let processed = 0;
  while (true) {
    const result = await migrationBatch(t, nextCursor);
    batches += 1;
    changedProfiles += result.changedProfiles;
    invalidActiveMacAuthorities += result.invalidActiveMacAuthorities;
    processed += result.processedProfiles;
    if (result.isDone) {
      return { batches, changedProfiles, invalidActiveMacAuthorities, processed };
    }
    nextCursor = result.continueCursor;
  }
}

async function profileFenceState(t: ReturnType<typeof testBackend>) {
  return await t.action(internal.internal.profileAuthSessionFenceInvariant.check, {});
}

async function storedFenceFields(t: ReturnType<typeof testBackend>) {
  return await t.run(async (ctx) => {
    const profiles = await ctx.db.query("tokenmaxxers").take(PROFILE_COUNT + 1);
    return profiles.map((profile) => ({
      activeAuthSessionId: profile.activeAuthSessionId,
      authSessionGeneration: profile.authSessionGeneration,
      authSubject: profile.authSubject,
    }));
  });
}

test("the Profile Auth Session fence migration is bounded, resumable, and idempotent", async () => {
  const t = testBackend();
  await seedProfiles(t);

  await expect(profileFenceState(t)).resolves.toEqual({
    invalidActiveMacAuthorities: 0,
    missingActiveAuthSessionIds: 61,
    missingAuthSessionGenerations: 61,
    profiles: PROFILE_COUNT,
    profilesMissingFenceFields: 91,
  });
  const legacyFields = await storedFenceFields(t);

  const firstBatch = await migrationBatch(t, null);
  expect(firstBatch).toMatchObject({
    changedProfiles: 19,
    invalidActiveMacAuthorities: 0,
    isDone: false,
    processedProfiles: 25,
  });
  const interruptedState = await profileFenceState(t);
  expect(interruptedState.profilesMissingFenceFields).toBeGreaterThan(0);
  expect(interruptedState.profilesMissingFenceFields).toBeLessThan(91);

  await expect(finishMigration(t, firstBatch.continueCursor)).resolves.toEqual({
    batches: 4,
    changedProfiles: 72,
    invalidActiveMacAuthorities: 0,
    processed: 96,
  });
  await expect(profileFenceState(t)).resolves.toEqual({
    invalidActiveMacAuthorities: 0,
    missingActiveAuthSessionIds: 0,
    missingAuthSessionGenerations: 0,
    profiles: PROFILE_COUNT,
    profilesMissingFenceFields: 0,
  });

  const firstRunFields = await storedFenceFields(t);
  expect(firstRunFields).toHaveLength(PROFILE_COUNT);
  expect(
    firstRunFields.filter(
      (profile, index) => JSON.stringify(profile) !== JSON.stringify(legacyFields[index]),
    ),
  ).toHaveLength(91);
  expect(firstRunFields.every((profile) => profile.activeAuthSessionId !== undefined)).toBe(true);
  expect(firstRunFields.every((profile) => profile.authSessionGeneration !== undefined)).toBe(true);

  await expect(finishMigration(t, null)).resolves.toEqual({
    batches: 5,
    changedProfiles: 0,
    invalidActiveMacAuthorities: 0,
    processed: PROFILE_COUNT,
  });
  await expect(storedFenceFields(t)).resolves.toEqual(firstRunFields);
  await expect(profileFenceState(t)).resolves.toEqual({
    invalidActiveMacAuthorities: 0,
    missingActiveAuthSessionIds: 0,
    missingAuthSessionGenerations: 0,
    profiles: PROFILE_COUNT,
    profilesMissingFenceFields: 0,
  });
});

test("the Profile Auth Session fence migration reports and preserves invalid authority", async () => {
  const t = testBackend();
  await t.run(async (ctx) => {
    const tokenmaxxerId = await ctx.db.insert("tokenmaxxers", {
      authSubject: "invalid-authority",
      createdAt: 0,
      displayName: "Invalid Authority",
      publicId: "TG-INVALID",
    });
    const activeDeviceId = await ctx.db.insert("devices", {
      createdAt: 0,
      generation: 1,
      installationCredentialDigest: "digest",
      lastSeenAt: 0,
      revokedAt: 1,
      tokenmaxxerId,
      usageBackfillCompletedAt: null,
    });
    await ctx.db.patch(tokenmaxxerId, { activeDeviceId });
  });

  await expect(migrationBatch(t, null)).resolves.toMatchObject({
    changedProfiles: 0,
    invalidActiveMacAuthorities: 1,
    isDone: true,
    processedProfiles: 1,
  });
  await expect(profileFenceState(t)).resolves.toEqual({
    invalidActiveMacAuthorities: 1,
    missingActiveAuthSessionIds: 1,
    missingAuthSessionGenerations: 1,
    profiles: 1,
    profilesMissingFenceFields: 1,
  });
});
