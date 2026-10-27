import typer

app = typer.Typer()


@app.command()
def report(path: str):
    print(f"writing {path}")
