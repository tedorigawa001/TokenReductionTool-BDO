#!/usr/bin/env bun
/**
 * Delete the Bushido test VM.
 * Usage: bun run scripts/benchmark/cleanup.ts
 */

import { vmDelete } from "./lib/vm";

console.log("Deleting bushido-test VM...");
await vmDelete();
console.log("Done.");
