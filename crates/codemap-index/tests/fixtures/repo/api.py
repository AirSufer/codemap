from fastapi import APIRouter
from svc import ChatService

router = APIRouter()

@router.post("/chat/stream")
def chat_stream(req, mystery):
    svc = ChatService()
    mystery.run()          # parameter receiver -> unresolvable, must be counted
    return svc.stream(req)
