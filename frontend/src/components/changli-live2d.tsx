import { useEffect, useMemo, useRef, useState } from "react";

type Mood = "idle" | "observing" | "satisfied" | "tease" | "cold";

type SystemState = "idle" | "downloading" | "complete";

const moodCopy: Record<Mood, string> = {
  idle: "又停住了。你是打算让我替你点开始吗？",
  observing: "队列在流动。我看着，别乱动。",
  satisfied: "完成了。勉强算得上像样。",
  tease: "点太勤了。你想让我夸你？",
  cold: "拖我也没用，节奏要按我的来。",
};

type ChangliLive2DProps = {
  systemState: SystemState;
  onTease?: () => void;
};

export function ChangliLive2D({ systemState, onTease }: ChangliLive2DProps) {
  const rootRef = useRef<HTMLDivElement | null>(null);
  const holdRef = useRef<number | null>(null);
  const dragOffset = useRef({ x: 0, y: 0 });
  const [position, setPosition] = useState({ x: 32, y: 26 });
  const [dragging, setDragging] = useState(false);
  const [mood, setMood] = useState<Mood>("idle");

  useEffect(() => {
    if (systemState === "downloading") setMood("observing");
    if (systemState === "complete") setMood("satisfied");
    if (systemState === "idle") setMood("idle");
  }, [systemState]);

  const bubbleText = useMemo(() => moodCopy[mood], [mood]);

  function onPointerDown(event: React.PointerEvent<HTMLButtonElement>) {
    const rect = rootRef.current?.getBoundingClientRect();
    if (!rect) return;
    dragOffset.current = { x: event.clientX - rect.left, y: event.clientY - rect.top };
    setDragging(true);
    holdRef.current = window.setTimeout(() => {
      setMood("cold");
      onTease?.();
    }, 750);
  }

  function onPointerMove(event: React.PointerEvent<HTMLDivElement>) {
    if (!dragging) return;
    const paneWidth = 260;
    const paneHeight = 320;
    const nextX = Math.max(8, Math.min(window.innerWidth - paneWidth - 10, event.clientX - dragOffset.current.x));
    const nextY = Math.max(8, Math.min(window.innerHeight - paneHeight - 10, event.clientY - dragOffset.current.y));
    setPosition({ x: nextX, y: nextY });
  }

  function onPointerUp() {
    setDragging(false);
    if (holdRef.current !== null) {
      window.clearTimeout(holdRef.current);
      holdRef.current = null;
    }
  }

  function onClick() {
    setMood("tease");
    onTease?.();
  }

  return (
    <div
      ref={rootRef}
      className="changli-live2d"
      style={{ left: `${position.x}px`, top: `${position.y}px` }}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
    >
      <div className="changli-live2d-shell">
        <button
          type="button"
          className="changli-live2d-core"
          onPointerDown={onPointerDown}
          onClick={onClick}
          title="长离交互核心"
        >
          <span className="changli-face">长离</span>
        </button>
        <div className="changli-status">
          <span className="chip">{systemState === "downloading" ? "观察中" : systemState === "complete" ? "轻微满意" : "无聊"}</span>
          <span className="chip subtle">可拖拽 / 可触摸</span>
        </div>
        <p className="changli-bubble">{bubbleText}</p>
      </div>
    </div>
  );
}
