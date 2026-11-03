// Wide distributed RAMs over the Mojo's serial port:
//   'W' a d -> 'K'  wa = a; m64[a%64] = d; mq[a%128] = d[0]; mr[a%128] = d[1];
//                   ms[a] = d[2]
//   'R' a   -> 3 bytes: m64[wa%64], m64[a%64],
//                       {3'b0, ms[wa], mr[a%128], mr[wa%128], mq[a%128], mq[wa%128]}
// m64 is read at both the write address and a second one (RAM64X1D), mq and
// mr are 128 x 1 dual-port (RAM128X1D), ms is 256 x 1 (RAM256X1S). Initial
// contents: m64[i] = 13i + 5, mq[i] = bit 2 of 7i + 3, mr[i] = bit 1 of
// 5i + 1, ms[i] = bit 3 of 11i + 7. Checked by tools/wideram-test.py.
module wideram (
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

    reg [7:0] m64 [0:63];
    reg       mq  [0:127];
    reg       mr  [0:127];
    reg       ms  [0:255];
    integer k, t;
    initial begin
        for (k = 0; k < 64; k = k + 1) m64[k] = 13 * k + 5;
        for (k = 0; k < 128; k = k + 1) begin
            t = 7 * k + 3; mq[k] = t[2];
            t = 5 * k + 1; mr[k] = t[1];
        end
        for (k = 0; k < 256; k = k + 1) begin
            t = 11 * k + 7; ms[k] = t[3];
        end
    end

    reg [7:0] wa = 0, ra = 0, data = 0;
    reg       we = 0;
    always @(posedge clk)
        if (we) begin
            m64[wa[5:0]] <= data;
            mq[wa[6:0]]  <= data[0];
            mr[wa[6:0]]  <= data[1];
            ms[wa]       <= data[2];
        end

    reg [7:0]  op = 0;
    reg [1:0]  nargs = 0;
    reg [7:0]  arg0 = 0;
    reg [23:0] out = 0;
    reg [1:0]  nout = 0;
    reg        reply = 0;
    wire       tx_taken;
    uart_tx #(.GAP_BITS(10)) utx (.clk(clk), .avr_busy(avr_rx_busy), .send(nout != 0 && ready),
                                  .data(out[7:0]), .taken(tx_taken), .tx_pin(avr_rx));
    always @(posedge clk) begin
        we    <= 1'b0;
        reply <= 1'b0;
        if (rx_valid) begin
            if (nargs == 0) begin
                // Unknown bytes are ignored (stray AVR bytes at startup).
                if (rx_data == "W" || rx_data == "R") begin
                    op <= rx_data; nargs <= 1;
                end
            end else if (nargs == 1) begin
                if (op == "R") begin
                    ra <= rx_data; nargs <= 0; reply <= 1'b1;
                end else begin
                    arg0 <= rx_data; nargs <= 2;
                end
            end else begin
                wa <= arg0; data <= rx_data; we <= 1'b1; nargs <= 0;
                out <= "K"; nout <= 1;
            end
        end
        if (reply) begin
            out  <= {3'b0, ms[wa], mr[ra[6:0]], mr[wa[6:0]], mq[ra[6:0]], mq[wa[6:0]],
                     m64[ra[5:0]], m64[wa[5:0]]};
            nout <= 3;
        end else if (tx_taken) begin
            out  <= {8'h00, out[23:8]};
            nout <= nout - 1'b1;
        end
    end
    assign led = m64[wa[5:0]];
endmodule
