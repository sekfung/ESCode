import type { Event, IDisposable } from "@escode/rpc";
import {
  conversationTopicFrameSchema,
  TopicWireFrameAssembler,
  type ConversationTopicFrame,
  type ConversationTopicWireCandidate,
} from "@escode/shared/escode-protocol-v4";
import type {
  IESCodeAgentService,
  ESCodeAgentSessionTarget,
} from "#src/escode-agent/escodeAgent.js";

const RESUBSCRIBE_DELAY_MS = 500;

/**
 * 持续的 V4 conversation 帧订阅（docs/specs/bots-v4-stream.md）：每个监听者一条独立的 background
 * 订阅，帧经装配后按序交付；gap/fault 时退订并重订阅，下一帧即新的 snapshot。dispose 后释放订阅，
 * 迟到的订阅 ACK 也要释放。
 */
export function observeConversationFrames(
  agent: Pick<
    IESCodeAgentService,
    "onDynamicConversationFrame" | "subscribeConversationV4" | "unsubscribeConversationV4"
  >,
  target: ESCodeAgentSessionTarget,
): Event<ConversationTopicFrame> {
  return (listener) => {
    let closed = false;
    let generation = 0;
    let subscriptionId: string | undefined;
    let seq: number | undefined;
    let assembler = new TopicWireFrameAssembler(conversationTopicFrameSchema);
    let early: ConversationTopicWireCandidate[] = [];
    let retry: ReturnType<typeof setTimeout> | undefined;
    const release = (id: string) => {
      void agent
        .unsubscribeConversationV4({ ...target, subscriptionId: id })
        .catch(() => undefined);
    };
    const restart = () => {
      if (subscriptionId) release(subscriptionId);
      subscriptionId = undefined;
      if (closed || retry) return;
      retry = setTimeout(() => {
        retry = undefined;
        subscribe();
      }, RESUBSCRIBE_DELAY_MS);
    };
    const accept = (wire: ConversationTopicWireCandidate): void => {
      if (closed || wire.topic !== `conversation/${target.sessionId}`) return;
      if (!subscriptionId) {
        // 订阅 ACK 可能晚于首帧；取得 ownership 前先缓存。
        if (early.length < 128) early.push(wire);
        return;
      }
      if (wire.subscriptionId !== subscriptionId) return;
      for (const event of assembler.accept(wire)) {
        const frame = event.kind === "fault" ? undefined : event.frame;
        // 装配器不校验 seq 连续：deltas 必须接在上一帧 toSeq 之后，否则视为 gap 重订阅。
        if (!frame || (frame.payload.kind === "deltas" && frame.fromSeq !== seq)) {
          restart();
          return;
        }
        seq = frame.toSeq;
        listener(frame);
      }
    };
    const frames: IDisposable = agent.onDynamicConversationFrame(target)(accept);
    const subscribe = () => {
      const current = ++generation;
      assembler = new TopicWireFrameAssembler(conversationTopicFrameSchema);
      early = [];
      seq = undefined;
      agent
        .subscribeConversationV4({ ...target, visibility: "background" })
        .then((result) => {
          if (closed || current !== generation) {
            release(result.ack.subscriptionId);
            return;
          }
          subscriptionId = result.ack.subscriptionId;
          for (const wire of early.splice(0)) accept(wire);
        })
        .catch(() => {
          if (current === generation) restart();
        });
    };
    subscribe();
    return {
      dispose() {
        closed = true;
        if (retry) clearTimeout(retry);
        frames.dispose();
        if (subscriptionId) release(subscriptionId);
        subscriptionId = undefined;
      },
    };
  };
}
