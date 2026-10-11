import assert from "node:assert/strict";
import { test } from "node:test";
import { SerialChunkBuffer } from "../src/serial/serialChunkBuffer.js";

function createBuffers() {
  let next = 0;
  const nextSeq = () => ++next;
  const options = {
    limitBytes: 6,
    coalesceWindowMs: 0,
    coalesceMaxBytes: 4096,
    onChunk: () => {},
    nextSeq,
  };
  return { a: new SerialChunkBuffer(options), b: new SerialChunkBuffer(options) };
}

test("多个缓冲共用 seq 分配器：seq 跨会话单调递增、互不重复", () => {
  const { a, b } = createBuffers();
  a.pushTx("user", Buffer.from("1"));
  b.pushTx("user", Buffer.from("2"));
  a.pushTx("user", Buffer.from("3"));
  assert.deepEqual(
    a.snapshot().chunks.map((chunk) => chunk.seq),
    [1, 3],
  );
  assert.equal(a.lastSeq, 3);
  assert.equal(b.lastSeq, 2);
});

test("seq 不连续时，仅在游标之后确有数据被淘汰或清空才标记 evicted", () => {
  const { a, b } = createBuffers();
  a.pushTx("user", Buffer.from("aaa")); // seq 1
  b.pushTx("user", Buffer.from("x")); // seq 2（其他会话）
  a.pushTx("user", Buffer.from("bbb")); // seq 3
  const intact = a.readSince({ sinceSeq: 0, direction: "both", maxBytes: 100 });
  assert.equal(intact.evicted, false);
  a.pushTx("user", Buffer.from("cc")); // seq 4，超过 6 字节淘汰 seq 1
  assert.equal(a.readSince({ sinceSeq: 0, direction: "both", maxBytes: 100 }).evicted, true);
  assert.equal(a.readSince({ sinceSeq: 1, direction: "both", maxBytes: 100 }).evicted, false);
  a.clear();
  const afterClear = a.readSince({ sinceSeq: 3, direction: "both", maxBytes: 100 });
  assert.equal(afterClear.evicted, true);
  assert.equal(afterClear.lastSeq, 4);
});
