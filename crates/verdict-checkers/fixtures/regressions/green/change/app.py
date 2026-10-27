from fastapi import FastAPI

app = FastAPI()


@app.get("/api/notes")
def list_notes():
    return [{"id": 1}]
