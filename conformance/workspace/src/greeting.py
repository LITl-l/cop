DEFAULT_TIMEOUT = 5.0

class Greeter:
    def greet(self, name: str) -> str:
        return f"hello {name}"

def greet(name):
    return Greeter().greet(name)
