import { createReadStream } from "node:fs";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { createZstdDecompress } from "node:zlib";
import { readZstdFrameChunks } from "./zstd-frame.js";

export interface CompleteLine {
  line: number;
  text: string;
  oversized: boolean;
}

export interface CompleteLineReadResult {
  completeLines: number;
  ignoredIncompleteTail: boolean;
  oversizedLines: number[];
}

const DEFAULT_MAX_LINE_CHARACTERS = 32 * 1024 * 1024;

/**
 * Reads only newline-terminated records. An active rollout's final, partial
 * write is intentionally retained nowhere and ignored until a later scan.
 */
export async function readCompleteLines(
  chunks: AsyncIterable<Uint8Array | string>,
  onLine: (line: CompleteLine) => void | Promise<void>,
  maxLineCharacters = DEFAULT_MAX_LINE_CHARACTERS,
): Promise<CompleteLineReadResult> {
  const decoder = new TextDecoder("utf-8");
  let carry = "";
  let completeLines = 0;
  let discardingOversizedLine = false;
  const oversizedLines: number[] = [];

  for await (const chunk of chunks) {
    carry += typeof chunk === "string" ? chunk : decoder.decode(chunk, { stream: true });

    let newlineIndex = carry.indexOf("\n");
    while (newlineIndex >= 0) {
      const piece = carry.slice(0, newlineIndex);
      carry = carry.slice(newlineIndex + 1);
      completeLines += 1;

      if (discardingOversizedLine || piece.length > maxLineCharacters) {
        oversizedLines.push(completeLines);
        discardingOversizedLine = false;
        await onLine({ line: completeLines, text: "", oversized: true });
      } else {
        await onLine({
          line: completeLines,
          text: piece.endsWith("\r") ? piece.slice(0, -1) : piece,
          oversized: false,
        });
      }
      newlineIndex = carry.indexOf("\n");
    }

    if (carry.length > maxLineCharacters) {
      carry = "";
      discardingOversizedLine = true;
    }
  }

  carry += decoder.decode();
  return {
    completeLines,
    ignoredIncompleteTail: carry.length > 0 || discardingOversizedLine,
    oversizedLines,
  };
}

export async function readCompleteFileLines(
  filePath: string,
  onLine: (line: CompleteLine) => void | Promise<void>,
  maxLineCharacters?: number,
): Promise<CompleteLineReadResult> {
  if (!filePath.endsWith(".jsonl.zst")) {
    return readCompleteLines(createReadStream(filePath), onLine, maxLineCharacters);
  }
  const decompressed = createZstdDecompress();
  const reading = readCompleteLines(decompressed, onLine, maxLineCharacters);
  // Node can ignore later frames that arrive in the same write. Keep frame
  // boundaries in separate writes and discard skippable metadata frames.
  // The framing reader also rejects truncated input before success is returned.
  const [, result] = await Promise.all([
    pipeline(Readable.from(readZstdFrameChunks(filePath)), decompressed),
    reading,
  ]);
  return result;
}
