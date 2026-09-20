import { describe, expect, it, vi } from "vitest";

import {
  readDashboardSnapshot,
  type DashboardAppServerClient,
} from "../src/core/dashboard-service.js";
import {
  AppServerProcessExitError,
  AppServerRpcError,
  CodexExecutableNotFoundError,
} from "../src/core/app-server/index.js";
import type { LocalUsageSummary } from "../src/core/types.js";

const LOCAL_USAGE: LocalUsageSummary = {
  generatedAt: "2026-08-27T00:00:00.000Z",
  sourceFiles: 1,
  indexedEvents: 2,
  skippedEvents: 0,
  filteredParentEvents: 1,
  total: {
    inputTokens: 10,
    cachedInputTokens: 4,
    outputTokens: 3,
    reasoningOutputTokens: 1,
    totalTokens: 13,
  },
  today: {
    inputTokens: 10,
    cachedInputTokens: 4,
    outputTokens: 3,
    reasoningOutputTokens: 1,
    totalTokens: 13,
  },
  byModel: [],
  byProject: [],
  warnings: [],
};

function fakeClient(
  overrides: Partial<DashboardAppServerClient> = {},
): DashboardAppServerClient {
  return {
    serverUserAgent: "codex-cli/0.150.0-alpha.8",
    accountRead: vi.fn(async () => ({
      account: { present: true },
      requiresOpenaiAuth: true,
    })),
    rateLimitsRead: vi.fn(async () => ({
      rateLimitsByLimitId: {
        codex: {
          limitId: "codex",
          limitName: "Codex",
          primary: {
            usedPercent: 25,
            windowDurationMins: 300,
            resetsAt: 1_800_000_000,
          },
          secondary: null,
        },
      },
      rateLimitResetCredits: null,
    })),
    usageRead: vi.fn(async () => ({
      summary: null,
      dailyUsageBuckets: [],
      threadUsage: null,
    })),
    close: vi.fn(),
    ...overrides,
  };
}

const FIXED_NOW = () => new Date("2026-08-27T01:02:03.000Z");

describe("readDashboardSnapshot", () => {
  it("combines normalized server data and local usage, then closes the client", async () => {
    const client = fakeClient();
    const snapshot = await readDashboardSnapshot({
      connect: async () => client,
      readLocalUsage: async () => LOCAL_USAGE,
      now: FIXED_NOW,
    });

    expect(snapshot.status).toBe("ready");
    expect(snapshot.codexVersion).toBe("0.150.0-alpha.8");
    expect(snapshot.quotaWindows).toHaveLength(1);
    expect(snapshot.planRenewalAt).toBeNull();
    expect(snapshot.planRenewalSource).toBeNull();
    expect(snapshot.localUsage).toBe(LOCAL_USAGE);
    expect(snapshot.fetchedAt).toBe("2026-08-27T01:02:03.000Z");
    expect(snapshot.accountDiagnostics).toMatchObject({
      readAt: "2026-08-27T01:02:03.000Z",
      methods: ["account/read", "account/rateLimits/read", "account/usage/read"],
    });
    expect(snapshot.accountDiagnostics?.durationMs).toBeGreaterThanOrEqual(0);
    expect(client.close).toHaveBeenCalledOnce();
  });

  it("keeps local usage available when Codex is not installed", async () => {
    const snapshot = await readDashboardSnapshot({
      connect: async () => { throw new CodexExecutableNotFoundError(); },
      readLocalUsage: async () => LOCAL_USAGE,
      now: FIXED_NOW,
    });

    expect(snapshot.status).toBe("unsupported");
    expect(snapshot.localUsage).toBe(LOCAL_USAGE);
    expect(snapshot.message).not.toContain("C:\\");
    expect(snapshot.accountDiagnostics).toMatchObject({
      readAt: "2026-08-27T01:02:03.000Z",
      methods: [],
    });
  });

  it("reports unauthenticated without calling protected usage methods", async () => {
    const client = fakeClient({
      accountRead: vi.fn(async () => ({ account: null, requiresOpenaiAuth: true })),
    });
    const snapshot = await readDashboardSnapshot({ connect: async () => client });

    expect(snapshot.status).toBe("unauthenticated");
    expect(client.rateLimitsRead).not.toHaveBeenCalled();
    expect(client.usageRead).not.toHaveBeenCalled();
    expect(snapshot.accountDiagnostics?.methods).toEqual(["account/read"]);
    expect(client.close).toHaveBeenCalledOnce();
  });

  it("audits only the three account-read methods", async () => {
    const snapshot = await readDashboardSnapshot({ connect: async () => fakeClient() });
    expect(snapshot.accountDiagnostics?.methods).toEqual([
      "account/read",
      "account/rateLimits/read",
      "account/usage/read",
    ]);
    expect(snapshot.accountDiagnostics?.methods).not.toContain("thread/start");
    expect(snapshot.accountDiagnostics?.methods).not.toContain("turn/start");
  });

  it("keeps a ready partial snapshot when only one server method fails", async () => {
    const client = fakeClient({
      usageRead: vi.fn(async () => {
        throw new AppServerRpcError("account/usage/read", {
          code: -32601,
          message: "Method not found",
        });
      }),
    });
    const snapshot = await readDashboardSnapshot({ connect: async () => client });

    expect(snapshot.status).toBe("ready");
    expect(snapshot.quotaWindows).toHaveLength(1);
    expect(snapshot.accountUsage).toBeNull();
    expect(snapshot.message).toBe("部分账号用量暂不可用。");
  });

  it("classifies two missing methods as unsupported", async () => {
    const missing = () => Promise.reject(new AppServerRpcError("test", {
      code: -32601,
      message: "Method not found",
    }));
    const client = fakeClient({
      rateLimitsRead: vi.fn(missing),
      usageRead: vi.fn(missing),
    });
    const snapshot = await readDashboardSnapshot({ connect: async () => client });

    expect(snapshot.status).toBe("unsupported");
    expect(snapshot.message).not.toContain("Method not found");
    expect(client.close).toHaveBeenCalledOnce();
  });

  it("maps an App Server exit to offline without exposing stderr", async () => {
    const client = fakeClient({
      accountRead: vi.fn(async () => {
        throw new AppServerProcessExitError({
          exitCode: 1,
          signal: null,
          stderr: "sensitive local diagnostic",
          stderrTruncated: false,
        });
      }),
    });
    const snapshot = await readDashboardSnapshot({ connect: async () => client });

    expect(snapshot.status).toBe("offline");
    expect(snapshot.message).not.toContain("sensitive");
    expect(client.close).toHaveBeenCalledOnce();
  });

  it("does not fail server data when the local index reader fails", async () => {
    const snapshot = await readDashboardSnapshot({
      connect: async () => fakeClient(),
      readLocalUsage: async () => { throw new Error("private path"); },
    });

    expect(snapshot.status).toBe("ready");
    expect(snapshot.localUsage).toBeNull();
    expect(snapshot.message).toBe("本机日志索引暂不可用。");
  });
});
