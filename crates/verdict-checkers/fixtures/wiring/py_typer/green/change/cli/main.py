import typer

from cli import export

app = typer.Typer()
app.add_typer(export.app, name="export")


@app.command()
def version():
    print("1.0")


if __name__ == "__main__":
    app()
