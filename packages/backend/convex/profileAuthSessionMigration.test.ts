/// <reference types="vite/client" />

import migrationsTest from "@convex-dev/migrations/test";
import { convexTest } from "convex-test";
import { expect, test } from "vitest";

import { internal } from "./_generated/api";
import schema from "./schema";

const modules = import.meta.glob("./**/*.ts");
const PROFILE_COUNT = 121;

type ProfileFencePageResult = {
  continueCursor: string;
  invalidActiveMacAuthorities: number;
  isDone: boolean;
  missingActiveAuthSessionIds: number;
  missingAuthSessionGenerations: number;
  processedProfiles: number;
  profilesMissingFenceFields: number;
};

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
    paginationOpts: {
      cursor,
      maximumRowsRead: 25,
      numItems: 25,
    },
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
  let cursor: string | null = null;
  let invalidActiveMacAuthorities = 0;
  let missingActiveAuthSessionIds = 0;
  let missingAuthSessionGenerations = 0;
  let profiles = 0;
  let profilesMissingFenceFields = 0;
  while (true) {
    const result: ProfileFencePageResult = await t.query(
      internal.internal.profileAuthSessionFenceInvariant.check,
      {
        paginationOpts: {
          cursor,
          maximumRowsRead: 100,
          numItems: 100,
        },
      },
    );
    invalidActiveMacAuthorities += result.invalidActiveMacAuthorities;
    missingActiveAuthSessionIds += result.missingActiveAuthSessionIds;
    missingAuthSessionGenerations += result.missingAuthSessionGenerations;
    profiles += result.processedProfiles;
    profilesMissingFenceFields += result.profilesMissingFenceFields;
    if (result.isDone) {
      return {
        invalidActiveMacAuthorities,
        missingActiveAuthSessionIds,
        missingAuthSessionGenerations,
        profiles,
        profilesMissingFenceFields,
      };
    }
    cursor = result.continueCursor;
  }
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

test("the Profile Auth Session fence operations reject an unbounded page", async () => {
  const t = testBackend();

  await expect(
    t.mutation(internal.internal.migrations.backfillProfileAuthSessionFence, {
      paginationOpts: {
        cursor: null,
        maximumRowsRead: 26,
        numItems: 25,
      },
    }),
  ).rejects.toThrow("Pagination maximumRowsRead must be between 1 and 25");
  await expect(
    t.query(internal.internal.profileAuthSessionFenceInvariant.check, {
      paginationOpts: {
        cursor: null,
        maximumRowsRead: 100,
        numItems: 101,
      },
    }),
  ).rejects.toThrow("Pagination numItems must be between 1 and 100");
});

test("the Profile Auth Session fence operations preserve native pagination options", async () => {
  const t = testBackend();
  await seedProfiles(t);

  const firstMigrationPage = await migrationBatch(t, null);
  await expect(
    t.mutation(internal.internal.migrations.backfillProfileAuthSessionFence, {
      paginationOpts: {
        cursor: null,
        endCursor: firstMigrationPage.continueCursor,
        id: 1,
        maximumBytesRead: 1_000_000,
        maximumRowsRead: 25,
        numItems: 1,
      },
    }),
  ).resolves.toMatchObject({
    changedProfiles: 0,
    processedProfiles: 25,
  });

  const firstInvariantPage: ProfileFencePageResult = await t.query(
    internal.internal.profileAuthSessionFenceInvariant.check,
    {
      paginationOpts: {
        cursor: null,
        maximumRowsRead: 25,
        numItems: 25,
      },
    },
  );
  await expect(
    t.query(internal.internal.profileAuthSessionFenceInvariant.check, {
      paginationOpts: {
        cursor: null,
        endCursor: firstInvariantPage.continueCursor,
        id: 2,
        maximumBytesRead: 1_000_000,
        maximumRowsRead: 100,
        numItems: 1,
      },
    }),
  ).resolves.toMatchObject({ processedProfiles: 25 });
});

test("the Profile Auth Session fence migration preserves an unclaimed Profile", async () => {
  const t = testBackend();
  await t.run(async (ctx) => {
    await ctx.db.insert("tokenmaxxers", {
      authSubject: "unclaimed-profile",
      createdAt: 0,
      displayName: "Unclaimed Profile",
      publicId: "TG-UNCLAIMED",
    });
  });

  await expect(profileFenceState(t)).resolves.toEqual({
    invalidActiveMacAuthorities: 0,
    missingActiveAuthSessionIds: 1,
    missingAuthSessionGenerations: 1,
    profiles: 1,
    profilesMissingFenceFields: 1,
  });
  await expect(migrationBatch(t, null)).resolves.toMatchObject({
    changedProfiles: 1,
    invalidActiveMacAuthorities: 0,
    isDone: true,
    processedProfiles: 1,
  });
  await expect(
    t.run(async (ctx) => {
      const profile = await ctx.db.query("tokenmaxxers").first();
      return {
        activeAuthSessionId: profile?.activeAuthSessionId,
        activeDeviceId: profile?.activeDeviceId,
        authSessionGeneration: profile?.authSessionGeneration,
      };
    }),
  ).resolves.toEqual({
    activeAuthSessionId: null,
    activeDeviceId: undefined,
    authSessionGeneration: 0,
  });
  await expect(profileFenceState(t)).resolves.toEqual({
    invalidActiveMacAuthorities: 0,
    missingActiveAuthSessionIds: 0,
    missingAuthSessionGenerations: 0,
    profiles: 1,
    profilesMissingFenceFields: 0,
  });
  await expect(migrationBatch(t, null)).resolves.toMatchObject({
    changedProfiles: 0,
    invalidActiveMacAuthorities: 0,
    isDone: true,
    processedProfiles: 1,
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
