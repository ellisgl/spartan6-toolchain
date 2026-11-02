// Wide-mux test over the Mojo's USB serial port: for each received byte a,
// replies with rom[a], rom[a ^ 8'h5a] (256-byte table: 8-input functions,
// LUT6s combined by MUXF7 + MUXF8) and rom7[a[6:0]] (128-byte table: 7-input
// functions, MUXF7 alone). Tables in rom_table.v. Checked by
// tools/rom-test.py.
module rom (
    input  wire       clk,
    input  wire       rst_n,
    input  wire       cclk,
    input  wire       avr_tx,
    output wire       avr_rx,
    input  wire       avr_rx_busy,
    output wire [7:0] led
);
    wire ready;
    mojo_ready rdy (.clk(clk), .cclk(cclk), .ready(ready));

    wire [7:0] rx_data;
    wire       rx_valid;
    uart_rx urx (.clk(clk), .rx_pin(avr_tx), .data(rx_data), .valid(rx_valid));

    reg  [7:0] addr = 0;
    wire [7:0] d0, d1;
    rom_table t0 (.addr(addr), .data(d0));
    rom_table t1 (.addr(addr ^ 8'h5a), .data(d1));
    wire [7:0] d2;
    rom_table7 t2 (.addr(addr[6:0]), .data(d2));

    reg [23:0] out = 0;
    reg [1:0]  nout = 0;
    reg        load = 0;
    wire       tx_taken;
    uart_tx #(.GAP_BITS(10)) utx (.clk(clk), .avr_busy(avr_rx_busy), .send(nout != 0 && ready && !load),
                                  .data(out[7:0]), .taken(tx_taken), .tx_pin(avr_rx));

    always @(posedge clk) begin
        load <= 1'b0;
        if (rx_valid) begin
            addr <= rx_data;
            load <= 1'b1;
        end
        if (load) begin
            out  <= {d2, d1, d0};
            nout <= 3;
        end else if (tx_taken) begin
            out  <= {8'h00, out[23:8]};
            nout <= nout - 1'b1;
        end
    end

    assign led = addr;
endmodule
