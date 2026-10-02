// Validate the Claude Code plugin and its marketplace, failing on any error or
// warning except one: the plugin deliberately has no `version`, so that every
// released commit counts as a new plugin version. That is how rolling releases
// reach plugin users.
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const repo = fileURLToPath(new URL("..", import.meta.url));
let failed = false;

for (const target of ["plugin", "."]) {
  const run = spawnSync("claude", ["plugin", "validate", "--json", target], {
    cwd: repo,
    encoding: "utf8",
  });
  let report;
  try {
    report = JSON.parse(run.stdout);
  } catch {
    console.error(`claude plugin validate ${target} did not report JSON:\n${run.stdout}${run.stderr}`);
    failed = true;
    continue;
  }
  const files = [report.manifest, ...(report.contents ?? [])].filter(Boolean);
  const problems = files.flatMap((file) => [
    ...(file.errors ?? []).map((e) => `${file.file}: error: ${e.path}: ${e.message}`),
    ...(file.warnings ?? [])
      .filter((w) => !(file.type === "plugin" && w.path === "version"))
      .map((w) => `${file.file}: warning: ${w.path}: ${w.message}`),
  ]);
  for (const problem of problems) console.error(problem);
  if (!report.success || problems.length > 0) {
    console.error(`claude plugin validate ${target} failed`);
    failed = true;
  } else {
    console.log(`claude plugin validate ${target}: ok`);
  }
}

process.exit(failed ? 1 : 0);
