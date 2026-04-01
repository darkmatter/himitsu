import { resolve, dirname } from "path";

const BIN =
  process.env.HIMITSU_BIN ??
  resolve(
    dirname(new URL(import.meta.url).pathname),
    "../../target/release/himitsu",
  );

export interface InitResult {
  data_dir: string;
  state_dir: string;
  store: string;
  pubkey: string;
  key_existed: boolean;
  store_existed: boolean;
  in_git_repo: boolean;
  suggested_remote: string | null;
  key_provider?: string;
}

export interface SearchResult {
  store: string;
  env: string;
  key: string;
}

export interface GroupInfo {
  name: string;
  count: number;
}

export interface RecipientInfo {
  group: string;
  name: string;
  pubkey: string;
}

/** Build the global `--store <path>` prefix when a store is specified. */
function storeArgs(store?: string): string[] {
  return store ? ["--store", store] : [];
}

function run(args: string[]): string {
  const result = Bun.spawnSync([BIN, ...args], {
    env: { ...process.env },
    stdout: "pipe",
    stderr: "pipe",
  });
  if (result.exitCode !== 0) {
    const stderr = result.stderr.toString().trim();
    throw new Error(stderr || `himitsu exited with code ${result.exitCode}`);
  }
  return result.stdout.toString().trim();
}

function tryRun(args: string[]): string | null {
  try {
    return run(args);
  } catch {
    return null;
  }
}

/** Run `himitsu init --json` and parse the structured output. */
export function init(store?: string): InitResult {
  const out = run([...storeArgs(store), "init", "--json"]);
  return JSON.parse(out) as InitResult;
}

export interface InitOptions {
  name?: string;
  home?: string;
  keyProvider?: string;
}

/** Run `himitsu init --json` with wizard-chosen options. */
export function initWithOptions(opts: InitOptions): InitResult {
  const args: string[] = ["init", "--json"];
  if (opts.name) args.push("--name", opts.name);
  if (opts.home) args.push("--home", opts.home);
  if (opts.keyProvider) args.push("--key-provider", opts.keyProvider);
  const out = run(args);
  return JSON.parse(out) as InitResult;
}

export function listEnvs(store?: string): string[] {
  const out = tryRun([...storeArgs(store), "ls"]);
  if (!out) return [];
  return out.split("\n").filter(Boolean);
}

export function listSecrets(env: string, store?: string): string[] {
  const out = tryRun([...storeArgs(store), "ls", env]);
  if (!out) return [];
  return out.split("\n").filter(Boolean);
}

export function getSecret(
  env: string,
  key: string,
  store?: string,
): string | null {
  return tryRun([...storeArgs(store), "get", env, key]);
}

export function setSecret(
  env: string,
  key: string,
  value: string,
  store?: string,
): string {
  return run([...storeArgs(store), "set", env, key, value]);
}

export function search(query: string, refresh = false): SearchResult[] {
  const args = ["search", query];
  if (refresh) args.push("--refresh");
  const out = tryRun(args);
  if (!out) return [];
  return out
    .split("\n")
    .filter(Boolean)
    .map((line) => {
      const [store, env, key] = line.split("\t");
      return { store, env, key };
    });
}

export function listGroups(store?: string): GroupInfo[] {
  const out = tryRun([...storeArgs(store), "group", "ls"]);
  if (!out) return [];
  return out
    .split("\n")
    .filter(Boolean)
    .map((line) => {
      const [name, rest] = line.split("\t");
      const count = parseInt(rest) || 0;
      return { name, count };
    });
}

export function listRecipients(store?: string): RecipientInfo[] {
  const out = tryRun([...storeArgs(store), "recipient", "ls"]);
  if (!out) return [];
  return out
    .split("\n")
    .filter(Boolean)
    .map((line) => {
      const [groupName, pubkey] = line.split("\t");
      const [group, name] = groupName.split("/");
      return { group, name, pubkey };
    });
}

export function encrypt(store?: string): string {
  return run([...storeArgs(store), "encrypt"]);
}
