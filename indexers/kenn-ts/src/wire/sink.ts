import {
  DEFAULT_FLUSH_BYTES,
  DEFAULT_FLUSH_FRAMES,
  type Frame,
} from "../../../frames";

/**
 * Buffered newline-delimited-JSON sink. Accumulates frames and flushes to the
 * provided writer when either the byte or frame threshold is crossed. One JSON
 * object per line, per the wire contract.
 */
export class JsonlSink {
  private buf: string[] = [];
  private bytes = 0;
  private frames = 0;

  constructor(
    private readonly writer: (chunk: string) => void,
    private readonly flushBytes: number = DEFAULT_FLUSH_BYTES,
    private readonly flushFrames: number = DEFAULT_FLUSH_FRAMES,
  ) {}

  push(frame: Frame): void {
    const line = JSON.stringify(frame) + "\n";
    this.buf.push(line);
    this.bytes += Buffer.byteLength(line, "utf8");
    this.frames += 1;
    if (this.bytes >= this.flushBytes || this.frames >= this.flushFrames) {
      this.flush();
    }
  }

  flush(): void {
    if (this.buf.length === 0) return;
    this.writer(this.buf.join(""));
    this.buf.length = 0;
    this.bytes = 0;
    this.frames = 0;
  }
}
