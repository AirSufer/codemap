import express from "express";
import { normalize } from "./util";

const app = express();

export class ChatService {
  streamResponse(req: Request) {
    const data = normalize(req);
    return this.finish(data);
  }
  finish(data: unknown) { return data; }
}

app.post("/chat/stream", function chatStream(req, res) {
  const svc = new ChatService();
  return svc.streamResponse(req);
});
