from fastapi import APIRouter
from .util import normalize

router = APIRouter()

class ChatService:
    def stream_response(self, req):
        data = normalize(req)
        return self.finish(data)

    def finish(self, data):
        return data

@router.post("/chat/stream")
def chat_stream(req):
    svc = ChatService()
    return svc.stream_response(req)
