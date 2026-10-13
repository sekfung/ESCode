import { describe, expect, it } from "vitest";
import { decodeEasyedaLog } from "../../src/circuit/domain/easyedaLog.js";

const record = (outer: unknown, inner: string): string => `${JSON.stringify(outer)}||${inner}|`;
const head = (uuid: string, client: string): string =>
  record({ type: "DOCHEAD" }, JSON.stringify({ docType: "SCH_PAGE", uuid, client }));

describe("EasyEDA V3 日志解码", () => {
  it("按 DOCHEAD 划分文档，DOCHEAD 之前的数据不属于任何文档", () => {
    const text = [
      record({ type: "META", ticket: 1 }, '{"name":"before"}'),
      head("p1", "c1"),
      record({ type: "WIRE", id: "w1", ticket: 1 }, '{"dots":[[0,0,10,0]]}'),
    ].join("\n");
    const documents = decodeEasyedaLog(text);
    expect(documents).toHaveLength(1);
    expect(documents[0]).toMatchObject({ docType: "SCH_PAGE", uuid: "p1", client: "c1" });
    expect(documents[0]!.records.map((entry) => entry.type)).toEqual(["WIRE"]);
  });

  it("同一 (type, id) 保留 ticket 最大的记录", () => {
    const text = [
      head("p1", "c1"),
      record({ type: "WIRE", id: "w1", ticket: 1 }, '{"value":"old"}'),
      record({ type: "WIRE", id: "w1", ticket: 5 }, '{"value":"new"}'),
      record({ type: "WIRE", id: "w1", ticket: 3 }, '{"value":"middle"}'),
    ].join("\n");
    const documents = decodeEasyedaLog(text);
    expect(documents[0]!.records).toHaveLength(1);
    expect(documents[0]!.records[0]!.payload["value"]).toBe("new");
  });

  it("ticket 相同时保留 client 较小的记录", () => {
    const text = [
      head("p1", "c2"),
      record({ type: "WIRE", id: "w1", ticket: 1 }, '{"value":"from-c2"}'),
      head("p1", "c1"),
      record({ type: "WIRE", id: "w1", ticket: 1 }, '{"value":"from-c1"}'),
    ].join("\n");
    const documents = decodeEasyedaLog(text);
    // 两份 DOCHEAD 是两份文档：各留一条，第二条文档的记录用更小的 client。
    expect(documents).toHaveLength(2);
    expect(documents[1]!.records[0]!.payload["value"]).toBe("from-c1");
  });

  it("内层为空串表示删除，落后于现有记录的删除不生效", () => {
    const text = [
      head("p1", "c1"),
      record({ type: "WIRE", id: "w1", ticket: 5 }, '{"value":"kept"}'),
      record({ type: "WIRE", id: "w1", ticket: 1 }, ""),
      record({ type: "WIRE", id: "w2", ticket: 2 }, '{"value":"gone"}'),
      record({ type: "WIRE", id: "w2", ticket: 3 }, ""),
    ].join("\n");
    const documents = decodeEasyedaLog(text);
    expect(documents[0]!.records.map((entry) => entry.id)).toEqual(["w1"]);
  });

  it("DELETE_DOC 丢弃整份文档", () => {
    const text = [
      head("p1", "c1"),
      record({ type: "WIRE", id: "w1", ticket: 1 }, "{}"),
      record({ type: "DELETE_DOC", ticket: 2 }, '{"isDelete":true}'),
      head("p2", "c1"),
      record({ type: "WIRE", id: "w2", ticket: 1 }, "{}"),
    ].join("\n");
    const documents = decodeEasyedaLog(text);
    expect(documents.map((document) => document.uuid)).toEqual(["p2"]);
  });

  it("坏行与缺 DOCHEAD 的数据都跳过，不猜", () => {
    const text = ["not json at all", "||{}|", head("p1", "c1"), "{\"type\":\"WIRE\"}||"].join("\n");
    const documents = decodeEasyedaLog(text);
    expect(documents).toHaveLength(1);
    expect(documents[0]!.records).toEqual([]);
  });
});
