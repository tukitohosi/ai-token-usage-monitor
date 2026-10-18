import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { constants, zstdCompressSync, zstdDecompressSync } from "node:zlib";
import { afterEach, describe, expect, it } from "vitest";
import { listRolloutFiles, rolloutKeyFromPath } from "../src/core/rollout/paths.js";
import { parseRolloutFile } from "../src/core/rollout/parser.js";
import { scanCodexRollouts } from "../src/core/rollout/aggregate.js";
import { validateZstdFrames } from "../src/core/rollout/zstd-frame.js";

const directories: string[] = [];
const uuid = "12345678-1234-1234-1234-123456789abc";
const filename = `rollout-2026-10-14T00-00-00-${uuid}.jsonl`;
const contents = [
  { type: "session_meta", payload: { id: uuid } },
  { type: "event_msg", timestamp: "2026-10-14T00:00:00Z", payload: {
    type: "token_count", info: { total_token_usage: {
      input_tokens: 100, cached_input_tokens: 50, output_tokens: 10,
      reasoning_output_tokens: 2, total_tokens: 110,
    } },
  } },
].map((record) => JSON.stringify(record)).join("\n") + "\n";

async function compressedFixture(bytes: Uint8Array): Promise<string> {
  const directory = await mkdtemp(path.join(tmpdir(), "usage-zstd-frame-"));
  directories.push(directory);
  const file = path.join(directory, `${filename}.zst`);
  await writeFile(file, bytes);
  return file;
}

function frameHeader(size: number, single: boolean, contentFlag: number, dictionaryFlag = 0): Buffer {
  const descriptor = (contentFlag << 6) | (single ? 32 : 0) | dictionaryFlag;
  const contentBytes = contentFlag === 0 ? (single ? 1 : 0) : 2 ** contentFlag;
  const sizeField = Buffer.alloc(contentBytes);
  if (contentBytes === 8) sizeField.writeBigUInt64LE(BigInt(size));
  else if (contentBytes > 0) sizeField.writeUIntLE(contentFlag === 1 ? size - 256 : size, 0, contentBytes);
  return Buffer.concat([
    Buffer.from([0x28, 0xb5, 0x2f, 0xfd, descriptor]),
    single ? Buffer.alloc(0) : Buffer.from([0]), // A 1 KiB window.
    Buffer.alloc([0, 1, 2, 4][dictionaryFlag]!), // Dictionary ID zero means no external dictionary.
    sizeField,
  ]);
}

function rawFrame(payload: Buffer, single = true, contentFlag = 0, dictionaryFlag = 0): Buffer {
  const block = Buffer.alloc(3);
  block.writeUIntLE((payload.length << 3) | 1, 0, 3);
  return Buffer.concat([frameHeader(payload.length, single, contentFlag, dictionaryFlag), block, payload]);
}

function skippableFrame(payload = Buffer.alloc(0)): Buffer {
  const header = Buffer.alloc(8);
  header.writeUInt32LE(0x184d2a5f, 0);
  header.writeUInt32LE(payload.length, 4);
  return Buffer.concat([header, payload]);
}

afterEach(async () => {
  await Promise.all(directories.splice(0).map((directory) => rm(directory, { recursive: true, force: true })));
});

describe("compressed Codex rollouts", () => {
  it("discovers archives and deduplicates compressed/plain copies by the same UUID", async () => {
    const home = await mkdtemp(path.join(tmpdir(), "usage-zstd-"));
    directories.push(home);
    const sessions = path.join(home, "sessions");
    const archive = path.join(home, "archived_sessions");
    await mkdir(sessions);
    await mkdir(archive);
    const plain = path.join(sessions, filename);
    const compressed = path.join(archive, `${filename}.zst`);
    await writeFile(plain, contents);
    await writeFile(compressed, zstdCompressSync(Buffer.from(contents)));
    expect(await listRolloutFiles(home)).toHaveLength(2);
    expect(rolloutKeyFromPath(plain)).toBe(uuid);
    expect(rolloutKeyFromPath(compressed)).toBe(uuid);
    const result = await scanCodexRollouts({ codexHome: home, timeZone: "UTC" });
    expect(result.summary.total.totalTokens).toBe(110);
  });

  it("rejects truncated compressed frames instead of accepting partial events", async () => {
    const home = await mkdtemp(path.join(tmpdir(), "usage-zstd-bad-"));
    directories.push(home);
    const compressed = zstdCompressSync(Buffer.from(contents));
    const file = path.join(home, `${filename}.zst`);
    await writeFile(file, compressed.subarray(0, compressed.length - 3));
    await expect(parseRolloutFile(file)).rejects.toThrow();
  });

  it.each([
    { single: false, flag: 0, size: 8 },
    { single: true, flag: 0, size: 8 },
    { single: false, flag: 1, size: 300 },
    { single: true, flag: 1, size: 300 },
    { single: false, flag: 2, size: 8 },
    { single: true, flag: 2, size: 8 },
    { single: false, flag: 3, size: 8 },
    { single: true, flag: 3, size: 8 },
  ])("accepts frame content-size flag $flag with single-segment=$single", async ({ single, flag, size }) => {
    const payload = Buffer.alloc(size, 65);
    for (const dictionaryFlag of [0, 1, 2, 3]) {
      const frame = rawFrame(payload, single, flag, dictionaryFlag);
      expect(zstdDecompressSync(frame)).toEqual(payload);
      await expect(validateZstdFrames(await compressedFixture(frame))).resolves.toBeUndefined();
    }
  });

  it("accepts raw and RLE blocks, including a non-final empty raw block", async () => {
    const payload = Buffer.from("aaaaaaaaaaaa");
    const emptyBlock = Buffer.alloc(3); // Non-final raw block, zero bytes.
    const rleBlock = Buffer.alloc(3);
    rleBlock.writeUIntLE((payload.length << 3) | 3, 0, 3);
    const frame = Buffer.concat([frameHeader(payload.length, true, 0), emptyBlock, rleBlock, Buffer.from("a")]);
    expect(zstdDecompressSync(frame)).toEqual(payload);
    await expect(validateZstdFrames(await compressedFixture(frame))).resolves.toBeUndefined();
  });

  it("reads concatenated frames separated by skippable frames as one JSONL stream", async () => {
    const payload = Buffer.from(contents);
    const middle = Math.floor(payload.length / 2);
    const bytes = Buffer.concat([
      skippableFrame(Buffer.from("metadata")),
      zstdCompressSync(Buffer.alloc(0)),
      zstdCompressSync(payload.subarray(0, middle), {
        params: { [constants.ZSTD_c_checksumFlag]: 1 },
      }),
      skippableFrame(), zstdCompressSync(payload.subarray(middle)), skippableFrame(),
    ]);
    const parsed = await parseRolloutFile(await compressedFixture(bytes));
    expect(parsed.usageEvents).toHaveLength(1);
    expect(parsed.usageEvents[0]!.usage.totalTokens).toBe(110);
  });

  it("accepts a valid skippable-only stream and headers split across read chunks", async () => {
    await expect(validateZstdFrames(await compressedFixture(skippableFrame()))).resolves.toBeUndefined();
    const bytes = Buffer.concat([
      skippableFrame(Buffer.alloc(65_527)), // The next magic crosses the default 64 KiB read boundary.
      zstdCompressSync(Buffer.from(contents)),
    ]);
    const parsed = await parseRolloutFile(await compressedFixture(bytes));
    expect(parsed.usageEvents[0]!.usage.totalTokens).toBe(110);
  });

  it("rejects every truncated prefix of a checksummed frame and corrupt checksums", async () => {
    const frame = zstdCompressSync(Buffer.from("x\n"), {
      params: { [constants.ZSTD_c_checksumFlag]: 1 },
    });
    const file = await compressedFixture(frame);
    for (let size = 0; size < frame.length; size += 1) {
      await writeFile(file, frame.subarray(0, size));
      await expect(validateZstdFrames(file)).rejects.toThrow();
    }
    await writeFile(file, frame);
    await expect(validateZstdFrames(file)).resolves.toBeUndefined();
    const corrupt = Buffer.from(frame);
    corrupt[corrupt.length - 1]! ^= 1;
    await writeFile(file, corrupt);
    await expect(parseRolloutFile(file)).rejects.toThrow();
  });

  it("rejects reserved descriptors, reserved block types, oversized blocks and trailing garbage", async () => {
    const reservedDescriptor = Buffer.from([0x28, 0xb5, 0x2f, 0xfd, 0x28, 0]);
    const reservedBlock = Buffer.concat([frameHeader(0, true, 0), Buffer.from([7, 0, 0])]);
    const oversized = rawFrame(Buffer.alloc(128 * 1024 + 1), true, 2);
    const trailing = Buffer.concat([zstdCompressSync(Buffer.from(contents)), Buffer.from([0])]);
    const shortSkippable = skippableFrame(Buffer.from("metadata")).subarray(0, 10);
    for (const bytes of [reservedDescriptor, reservedBlock, oversized, trailing, shortSkippable]) {
      await expect(validateZstdFrames(await compressedFixture(bytes))).rejects.toThrow();
    }
  });
});
