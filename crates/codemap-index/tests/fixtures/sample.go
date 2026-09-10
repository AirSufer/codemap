package chat

import "net/http"

type ChatService struct{}

func (s *ChatService) Stream(w http.ResponseWriter, r *http.Request) {
	s.finish(w)
}

func (s *ChatService) finish(w http.ResponseWriter) {}

func Register(mux *http.ServeMux) {
	svc := &ChatService{}
	mux.HandleFunc("/chat/stream", svc.Stream)
}
