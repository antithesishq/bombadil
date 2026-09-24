// Runs the browser side of the "introduction" protocol (see index.html) and
// reports every message back to the page, which renders the conversation.
onmessage = ({ data: { wsUrl, fullName } }) => {
    const log = (who, text) => postMessage({ who, text });

    const socket = new WebSocket(wsUrl);

    socket.onopen = () => log("browser", "connected");

    socket.onmessage = (event) => {
        log("server", event.data);
        if (event.data === "What's your name?: ") {
            const answer = `My name is ${fullName}`;
            log("browser", answer);
            socket.send(answer);
        }
    };

    socket.onerror = () => log("browser", "websocket error");
    socket.onclose = (event) => log("browser", `disconnected (code ${event.code})`);
};
