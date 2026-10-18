import { createReadStream } from "node:fs";

/**
 * Some Node releases accept truncated Zstd input. Check frame boundaries too;
 * the native decoder still validates payloads, content sizes and checksums.
 * Yield encoded chunks that never cross frames and omit skippable frames, so
 * Node also processes concatenated frames that originally share a disk chunk.
 */
export async function* readZstdFrameChunks(filePath: string): AsyncGenerator<Buffer> {
  type Stage = "magic" | "descriptor" | "block" | "skip-size";
  let stage: Stage = "magic";
  let header = Buffer.alloc(0);
  let needed = 4;
  let skip = 0;
  let nextStage: Stage = "magic";
  let checksum = false;
  let frames = 0;
  let emitPayload = true;
  const invalid = () => new Error("Incomplete or invalid Zstandard frame");

  for await (const chunk of createReadStream(filePath)) {
    const bytes = chunk as Buffer;
    let offset = 0;
    while (offset < bytes.length) {
      if (skip > 0) {
        const count = Math.min(skip, bytes.length - offset);
        if (emitPayload) yield bytes.subarray(offset, offset + count);
        skip -= count;
        offset += count;
        if (skip > 0) continue;
        stage = nextStage;
        needed = stage === "block" ? 3 : 4;
        continue;
      }
      const count = Math.min(needed - header.length, bytes.length - offset);
      header = Buffer.concat([header, bytes.subarray(offset, offset + count)]);
      offset += count;
      if (header.length < needed) continue;
      if (stage === "magic") {
        const magic = header.readUInt32LE();
        if (magic === 0xfd2fb528) {
          stage = "descriptor";
          needed = 1;
          frames += 1;
          emitPayload = true;
          yield header;
        } else if (magic >= 0x184d2a50 && magic <= 0x184d2a5f) {
          stage = "skip-size";
          needed = 4;
          frames += 1;
          emitPayload = false;
        } else throw invalid();
      } else if (stage === "descriptor") {
        const descriptor = header[0]!;
        if (descriptor & 8) throw invalid();
        const singleSegment = (descriptor & 32) !== 0;
        const contentFlag = descriptor >>> 6;
        const contentSizeBytes = contentFlag === 0 ? (singleSegment ? 1 : 0) : 2 ** contentFlag;
        checksum = (descriptor & 4) !== 0;
        skip = (singleSegment ? 0 : 1) + [0, 1, 2, 4][descriptor & 3]! + contentSizeBytes;
        nextStage = "block";
        stage = "block";
        needed = 3;
        yield header;
      } else if (stage === "skip-size") {
        skip = header.readUInt32LE();
        nextStage = "magic";
        stage = "magic";
        needed = 4;
      } else {
        const block = header.readUIntLE(0, 3);
        const blockType = (block >>> 1) & 3;
        const blockSize = block >>> 3;
        if (blockType === 3 || blockSize > 128 * 1024) throw invalid();
        skip = blockType === 1 ? 1 : blockSize;
        const last = (block & 1) !== 0;
        if (last && checksum) skip += 4;
        nextStage = last ? "magic" : "block";
        stage = nextStage;
        needed = stage === "block" ? 3 : 4;
        yield header;
      }
      header = Buffer.alloc(0);
    }
  }
  if (skip > 0 || header.length > 0 || stage !== "magic" || frames === 0) throw invalid();
}

export async function validateZstdFrames(filePath: string): Promise<void> {
  for await (const _chunk of readZstdFrameChunks(filePath)) {
    // Boundary validation only. No decompressed/source text is retained here.
  }
}
