import * as vscode from "vscode";
import { execFile } from "child_process";

/**
 * Runs `termlab check` on Lua plugin files and maps the output to
 * VS Code diagnostics (squiggly underlines, Problems panel).
 *
 * Output format from `termlab check`:
 *   file.lua:7:1: error: session.exec() expects 1 argument(s), got 2
 *   file.lua: warning: missing plugin-description header comment
 */
export class TermLabDiagnostics implements vscode.Disposable {
  private collection: vscode.DiagnosticCollection;
  private running = new Map<string, AbortController>();

  constructor() {
    this.collection =
      vscode.languages.createDiagnosticCollection("termlab");
  }

  dispose() {
    this.collection.dispose();
    for (const controller of this.running.values()) {
      controller.abort();
    }
  }

  clear(uri: vscode.Uri) {
    this.collection.delete(uri);
  }

  check(doc: vscode.TextDocument) {
    const uri = doc.uri;
    const fsPath = uri.fsPath;

    // Cancel any in-flight check for this file.
    const prev = this.running.get(fsPath);
    if (prev) {
      prev.abort();
    }

    const controller = new AbortController();
    this.running.set(fsPath, controller);

    const config = vscode.workspace.getConfiguration("termlab");
    const executable = config.get<string>("executablePath", "termlab");

    execFile(
      executable,
      ["check", fsPath],
      { signal: controller.signal, timeout: 10_000 },
      (error, stdout, stderr) => {
        this.running.delete(fsPath);

        if (controller.signal.aborted) {
          return;
        }

        // `termlab check` writes diagnostics to stderr for errors/warnings,
        // and "file: ok" to stdout for clean files.
        const output = (stderr || "") + (stdout || "");
        const diagnostics = parseDiagnostics(output, doc);
        this.collection.set(uri, diagnostics);
      }
    );
  }
}

// Matches: path:line:col: severity: message
// or:      path:severity: message (line=0, no col)
const DIAG_RE =
  /^(.+?):(?:(\d+):(\d+):\s*)?(error|warning):\s*(.+)$/;

function parseDiagnostics(
  output: string,
  doc: vscode.TextDocument
): vscode.Diagnostic[] {
  const diagnostics: vscode.Diagnostic[] = [];

  for (const rawLine of output.split("\n")) {
    const line = rawLine.trim();
    if (!line || line.endsWith(": ok")) {
      continue;
    }

    const match = DIAG_RE.exec(line);
    if (!match) {
      continue;
    }

    const [, , lineStr, colStr, severity, message] = match;
    const lineNum = lineStr ? parseInt(lineStr, 10) - 1 : 0;
    const colNum = colStr ? parseInt(colStr, 10) - 1 : 0;

    // Highlight the whole line if we have a line number.
    const range =
      lineNum >= 0 && lineNum < doc.lineCount
        ? doc.lineAt(lineNum).range
        : new vscode.Range(0, 0, 0, 0);

    const diagSeverity =
      severity === "error"
        ? vscode.DiagnosticSeverity.Error
        : vscode.DiagnosticSeverity.Warning;

    const diag = new vscode.Diagnostic(range, message, diagSeverity);
    diag.source = "termlab";
    diagnostics.push(diag);
  }

  return diagnostics;
}
